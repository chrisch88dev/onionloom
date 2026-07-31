//! Paired-coordinate candidate generation with simultaneous field division.

use std::hint::black_box;
use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use curve25519_dalek::{EdwardsPoint, Scalar};

use crate::crypto::keys::ExpandedSecret;
use crate::search::filter::FilterSet;

use super::field::FieldElement;
use super::point::{AffinePoint, ExtendedPoint};

/// Reusable affine multiples of the cofactored Ed25519 base point.
pub(crate) struct OffsetTable {
    batch_size: usize,
    points: Box<[AffinePoint]>,
}

impl OffsetTable {
    /// Builds the shared table for an even candidate batch size.
    pub(crate) fn new(batch_size: usize) -> Result<Arc<Self>> {
        ensure!(
            batch_size > 0 && batch_size % 2 == 0,
            "symmetry batch size must be positive and even"
        );
        let points = match batch_size / 2 {
            32 => build_offsets::<32>()?,
            64 => build_offsets::<64>()?,
            128 => build_offsets::<128>()?,
            256 => build_offsets::<256>()?,
            512 => build_offsets::<512>()?,
            1024 => build_offsets::<1024>()?,
            2048 => build_offsets::<2048>()?,
            4096 => build_offsets::<4096>()?,
            _ => anyhow::bail!("unsupported symmetry batch size {batch_size}"),
        };
        Ok(Arc::new(Self {
            batch_size,
            points: points.into_boxed_slice(),
        }))
    }

    /// Returns the number of paired candidates, excluding the center.
    pub(crate) const fn batch_size(&self) -> usize {
        self.batch_size
    }
}

/// Mutable buffers and center state for one disjoint worker range.
pub(crate) struct SymmetryEngine {
    secret: ExpandedSecret,
    table: Arc<OffsetTable>,
    center: AffinePoint,
    stride: ExtendedPoint,
    numerators: Box<[FieldElement]>,
    denominators: Box<[FieldElement]>,
    quotients: Box<[FieldElement]>,
    block_start: u64,
    block_stride: u64,
}

impl SymmetryEngine {
    /// Creates one worker whose blocks interleave with the other workers.
    pub(crate) fn new(
        secret: &ExpandedSecret,
        table: Arc<OffsetTable>,
        workers: usize,
        worker: usize,
        block_start: u64,
    ) -> Result<Self> {
        ensure!(workers > 0, "worker count must be greater than zero");
        ensure!(worker < workers, "worker index must be in range");
        let candidates_per_block = table
            .batch_size
            .checked_add(1)
            .context("symmetry candidate count overflow")?;
        let half = table.batch_size / 2;
        let center_offset = block_start
            .checked_add(u64::try_from(half).expect("half batch fits u64"))
            .context("symmetry center offset overflow")?;
        let center =
            AffinePoint::decompress(secret.point_at_offset(center_offset).compress().to_bytes())?;
        let block_stride = u64::try_from(workers)
            .and_then(|count| {
                u64::try_from(candidates_per_block).map(|batch| count.saturating_mul(batch))
            })
            .expect("worker block stride fits u64");
        let stride_scalar = Scalar::from(block_stride) * Scalar::from(8_u64);
        let stride =
            AffinePoint::decompress(EdwardsPoint::mul_base(&stride_scalar).compress().to_bytes())?
                .to_extended();
        let division_count = candidates_per_block;
        Ok(Self {
            secret: secret.clone(),
            table,
            center,
            stride,
            numerators: vec![FieldElement::ZERO; division_count].into_boxed_slice(),
            denominators: vec![FieldElement::ZERO; division_count].into_boxed_slice(),
            quotients: vec![FieldElement::ZERO; division_count].into_boxed_slice(),
            block_start,
            block_stride,
        })
    }

    /// Returns the first offset in the next block.
    pub(crate) const fn block_start(&self) -> u64 {
        self.block_start
    }

    /// Generates one complete block and visits every public y coordinate.
    pub(crate) fn generate(&mut self, mut visit: impl FnMut([u8; 32], u64)) -> Result<usize> {
        let batch_size = self.table.batch_size;
        let half = batch_size / 2;
        let next_center_projective = self.center.to_extended().add(self.stride);

        for (index, offset) in self.table.points.iter().enumerate() {
            let x1_y2 = self.center.x.multiply(offset.y);
            let y1_x2 = self.center.y.multiply(offset.x);
            self.numerators[index] = self.center.xy.subtract(offset.xy);
            self.denominators[index] = x1_y2.subtract(y1_x2);
            self.numerators[half + index] = self.center.xy.add(offset.xy);
            self.denominators[half + index] = x1_y2.add(y1_x2);
        }
        self.numerators[batch_size] = FieldElement::ONE;
        self.denominators[batch_size] = next_center_projective.z;

        if self.denominators.iter().copied().any(FieldElement::is_zero) {
            return self.generate_exceptional(&mut visit);
        }

        simultaneous_division::<16>(&self.numerators, &self.denominators, &mut self.quotients);

        for index in 0..half {
            let plus_offset = self
                .block_start
                .checked_add(u64::try_from(half + 1 + index).expect("batch index fits u64"))
                .context("symmetry plus offset overflow")?;
            visit(self.quotients[index].to_bytes(), plus_offset);
            let minus_offset = self
                .block_start
                .checked_add(u64::try_from(half - 1 - index).expect("batch index fits u64"))
                .context("symmetry minus offset overflow")?;
            visit(self.quotients[half + index].to_bytes(), minus_offset);
        }
        let center_offset = self
            .block_start
            .checked_add(u64::try_from(half).expect("half batch fits u64"))
            .context("symmetry center offset overflow")?;
        visit(self.center.y.to_bytes(), center_offset);

        self.center = next_center_projective.to_affine(self.quotients[batch_size]);
        self.block_start = self
            .block_start
            .checked_add(self.block_stride)
            .context("symmetry worker exhausted the offset range")?;
        Ok(batch_size + 1)
    }

    fn generate_exceptional(&mut self, visit: &mut impl FnMut([u8; 32], u64)) -> Result<usize> {
        let candidates = self.table.batch_size + 1;
        for index in 0..candidates {
            let offset = self
                .block_start
                .checked_add(u64::try_from(index).expect("batch index fits u64"))
                .context("exceptional symmetry offset overflow")?;
            let mut public = self.secret.with_offset(offset)?.public_key();
            public[31] &= 0x7f;
            visit(public, offset);
        }
        self.block_start = self
            .block_start
            .checked_add(self.block_stride)
            .context("symmetry worker exhausted the offset range")?;
        let half = self.table.batch_size / 2;
        let center_offset = self
            .block_start
            .checked_add(u64::try_from(half).expect("half batch fits u64"))
            .context("exceptional symmetry center offset overflow")?;
        self.center = AffinePoint::decompress(
            self.secret
                .point_at_offset(center_offset)
                .compress()
                .to_bytes(),
        )?;
        Ok(candidates)
    }

    /// Runs one benchmark block with an optional real prefix filter.
    pub(crate) fn benchmark_block(&mut self, filters: Option<&FilterSet>) -> Result<usize> {
        self.generate(|public_y, _| {
            if let Some(filters) = filters {
                black_box(filters.matches_y_coordinate(&public_y));
            } else {
                black_box(public_y);
            }
        })
    }
}

fn simultaneous_division<const CHAINS: usize>(
    numerators: &[FieldElement],
    denominators: &[FieldElement],
    output: &mut [FieldElement],
) {
    debug_assert_eq!(numerators.len(), denominators.len());
    debug_assert_eq!(numerators.len(), output.len());
    debug_assert!(CHAINS > 0);
    debug_assert!(denominators.len() >= CHAINS);

    let mut products = [FieldElement::ONE; CHAINS];
    for base in (0..denominators.len()).step_by(CHAINS) {
        for (chain, product) in products.iter_mut().enumerate() {
            let index = base + chain;
            if index < denominators.len() {
                output[index] = product.multiply(numerators[index]);
                *product = product.multiply(denominators[index]);
            }
        }
    }

    let mut prefixes = [FieldElement::ONE; CHAINS];
    let mut combined = products[0];
    for (prefix, product) in prefixes[1..].iter_mut().zip(&products[1..]) {
        *prefix = combined;
        combined = combined.multiply(*product);
    }
    let mut combined_inverse = combined.invert();
    let mut inverses = [FieldElement::ZERO; CHAINS];
    for ((inverse, prefix), product) in inverses[1..]
        .iter_mut()
        .zip(&prefixes[1..])
        .zip(&products[1..])
        .rev()
    {
        *inverse = combined_inverse.multiply(*prefix);
        combined_inverse = combined_inverse.multiply(*product);
    }
    inverses[0] = combined_inverse;

    let rounds = denominators.len().div_ceil(CHAINS);
    for round in (0..rounds).rev() {
        let base = round * CHAINS;
        for (chain, inverse) in inverses.iter_mut().enumerate() {
            let index = base + chain;
            if index < denominators.len() {
                output[index] = inverse.multiply(output[index]);
                *inverse = inverse.multiply(denominators[index]);
            }
        }
    }
}

fn build_offsets<const N: usize>() -> Result<Vec<AffinePoint>> {
    let step = EdwardsPoint::mul_base(&Scalar::from(8_u64));
    let mut point = step;
    let points: [EdwardsPoint; N] = std::array::from_fn(|_| {
        let output = point;
        point += step;
        output
    });
    EdwardsPoint::compress_batch(&points)
        .into_iter()
        .map(|compressed| AffinePoint::decompress(compressed.to_bytes()))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::crypto::keys::ExpandedSecret;

    use super::{OffsetTable, SymmetryEngine};

    fn fixed_secret() -> ExpandedSecret {
        let mut expanded = [0_u8; 64];
        expanded[0] = 0x18;
        expanded[31] = 0x40;
        ExpandedSecret::from_bytes(expanded).expect("valid fixed test scalar")
    }

    #[test]
    fn deterministic_candidate_ranges_match_dalek_without_gaps() {
        const BATCH_SIZE: usize = 64;
        const CANDIDATES_PER_BLOCK: u64 = 65;
        const HALF_BATCH: u64 = 32;
        const SEARCH_START: u64 = 12_345;
        const WORKERS: usize = 3;
        const ROUNDS: u64 = 2;

        let secret = fixed_secret();
        let table = OffsetTable::new(64).expect("offset table");
        let mut observed = BTreeMap::new();
        for worker in 0..WORKERS {
            let worker = u64::try_from(worker).expect("worker fits u64");
            let start = SEARCH_START + worker * CANDIDATES_PER_BLOCK;
            let mut engine = SymmetryEngine::new(
                &secret,
                table.clone(),
                WORKERS,
                usize::try_from(worker).expect("worker fits usize"),
                start,
            )
            .expect("engine");

            for round in 0..ROUNDS {
                let block_start = start
                    + round
                        * u64::try_from(WORKERS).expect("workers fit u64")
                        * CANDIDATES_PER_BLOCK;
                assert_eq!(engine.block_start(), block_start);

                let mut callback_offsets = Vec::with_capacity(BATCH_SIZE + 1);
                let generated = engine
                    .generate(|public_y, offset| {
                        let mut expected = secret.with_offset(offset).expect("offset").public_key();
                        expected[31] &= 0x7f;
                        assert_eq!(public_y, expected, "offset {offset}");
                        assert!(observed.insert(offset, public_y).is_none());
                        callback_offsets.push(offset);
                    })
                    .expect("symmetry batch");
                assert_eq!(generated, BATCH_SIZE + 1);

                let mut expected_callback_offsets = Vec::with_capacity(BATCH_SIZE + 1);
                for index in 0..HALF_BATCH {
                    expected_callback_offsets.push(block_start + HALF_BATCH + 1 + index);
                    expected_callback_offsets.push(block_start + HALF_BATCH - 1 - index);
                }
                expected_callback_offsets.push(block_start + HALF_BATCH);
                assert_eq!(callback_offsets, expected_callback_offsets);

                let mut sorted_block_offsets = callback_offsets;
                sorted_block_offsets.sort_unstable();
                assert_eq!(
                    sorted_block_offsets,
                    (block_start..block_start + CANDIDATES_PER_BLOCK).collect::<Vec<_>>()
                );
                assert_eq!(
                    engine.block_start(),
                    block_start
                        + u64::try_from(WORKERS).expect("workers fit u64") * CANDIDATES_PER_BLOCK
                );
            }
        }
        let candidate_count =
            u64::try_from(WORKERS).expect("workers fit u64") * ROUNDS * CANDIDATES_PER_BLOCK;
        assert_eq!(
            observed.len(),
            usize::try_from(candidate_count).expect("candidate count fits usize")
        );
        assert_eq!(
            observed.keys().copied().collect::<Vec<_>>(),
            (SEARCH_START..SEARCH_START + candidate_count).collect::<Vec<_>>()
        );
    }
}
