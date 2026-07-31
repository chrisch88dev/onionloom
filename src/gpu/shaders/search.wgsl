const WORKGROUP_SIZE: u32 = 128u;
const OFFSET_PAIRS: u32 = 127u;
const CANDIDATES_PER_GROUP: u32 = 255u;
const RESULT_CAPACITY: u32 = 256u;
const LOW_25_BITS: u64 = 0x1fffffflu;
const LOW_26_BITS: u64 = 0x3fffffflu;

struct FieldElement {
    limbs: array<u32, 10>,
}

struct Point {
    x: FieldElement,
    y: FieldElement,
    z: FieldElement,
    t: FieldElement,
}

struct CompressedPoint {
    words: array<u32, 8>,
}

struct PatternMask {
    value: array<u32, 8>,
    mask: array<u32, 8>,
}

struct SearchConfig {
    workgroup_count: u32,
    pattern_count: u32,
    rounds: u32,
    base_low: u32,
    base_high: u32,
    group_start: u32,
    group_count: u32,
    source_bank: u32,
    destination_bank: u32,
    reserved_0: u32,
    reserved_1: u32,
    reserved_2: u32,
}

struct SearchResult {
    found: atomic<u32>,
    arithmetic_error: atomic<u32>,
    offsets: array<vec2<u32>, 256>,
}

const EDWARDS_D: FieldElement = FieldElement(array<u32, 10>(
    56195235u,
    13857412u,
    51736253u,
    6949390u,
    114729u,
    24766616u,
    60832955u,
    30306712u,
    48412415u,
    21499315u,
));

const EDWARDS_D2: FieldElement = FieldElement(array<u32, 10>(
    45281625u,
    27714825u,
    36363642u,
    13898781u,
    229458u,
    15978800u,
    54557047u,
    27058993u,
    29715967u,
    9444199u,
));

const SQRT_MINUS_ONE: FieldElement = FieldElement(array<u32, 10>(
    34513072u,
    25610706u,
    9377949u,
    3500415u,
    12389472u,
    33281959u,
    41962654u,
    31548777u,
    326685u,
    11406482u,
));

@group(0) @binding(0)
var<storage, read> compressed_points: array<CompressedPoint>;

@group(0) @binding(1)
var<storage, read_write> points: array<Point>;

@group(0) @binding(2)
var<storage, read> patterns: array<PatternMask>;

@group(0) @binding(3)
var<uniform> config: SearchConfig;

@group(0) @binding(4)
var<storage, read_write> search_result: SearchResult;

var<workgroup> product_tree: array<FieldElement, 512>;
var<workgroup> center_point: Point;
var<workgroup> next_center_projective: Point;
var<workgroup> batch_valid: u32;

fn field_zero() -> FieldElement {
    return FieldElement(array<u32, 10>(0u, 0u, 0u, 0u, 0u, 0u, 0u, 0u, 0u, 0u));
}

fn field_one() -> FieldElement {
    return FieldElement(array<u32, 10>(1u, 0u, 0u, 0u, 0u, 0u, 0u, 0u, 0u, 0u));
}

fn field_reduce(input: array<u64, 10>) -> FieldElement {
    var z = input;
    z[1] += z[0] >> 26u;
    z[0] &= LOW_26_BITS;
    z[5] += z[4] >> 26u;
    z[4] &= LOW_26_BITS;

    z[2] += z[1] >> 25u;
    z[1] &= LOW_25_BITS;
    z[6] += z[5] >> 25u;
    z[5] &= LOW_25_BITS;

    z[3] += z[2] >> 26u;
    z[2] &= LOW_26_BITS;
    z[7] += z[6] >> 26u;
    z[6] &= LOW_26_BITS;

    z[4] += z[3] >> 25u;
    z[3] &= LOW_25_BITS;
    z[8] += z[7] >> 25u;
    z[7] &= LOW_25_BITS;

    z[5] += z[4] >> 26u;
    z[4] &= LOW_26_BITS;
    z[9] += z[8] >> 26u;
    z[8] &= LOW_26_BITS;

    z[0] += 19lu * (z[9] >> 25u);
    z[9] &= LOW_25_BITS;
    z[1] += z[0] >> 26u;
    z[0] &= LOW_26_BITS;

    var output = field_zero();
    for (var index = 0u; index < 10u; index += 1u) {
        output.limbs[index] = u32(z[index]);
    }
    return output;
}

fn field_add(left: FieldElement, right: FieldElement) -> FieldElement {
    var coefficients: array<u64, 10>;
    for (var index = 0u; index < 10u; index += 1u) {
        coefficients[index] = u64(left.limbs[index]) + u64(right.limbs[index]);
    }
    return field_reduce(coefficients);
}

fn field_subtract(left: FieldElement, right: FieldElement) -> FieldElement {
    var coefficients: array<u64, 10>;
    for (var index = 0u; index < 10u; index += 1u) {
        let modulus_limb = select(0x1ffffffu, 0x3ffffffu, (index & 1u) == 0u);
        if index == 0u {
            coefficients[index] =
                u64(left.limbs[index] + (0x3ffffedu << 4u) - right.limbs[index]);
        } else {
            coefficients[index] =
                u64(left.limbs[index] + (modulus_limb << 4u) - right.limbs[index]);
        }
    }
    return field_reduce(coefficients);
}

fn field_negate(value: FieldElement) -> FieldElement {
    return field_subtract(field_zero(), value);
}

fn multiply_term(left: u32, right: u32) -> u64 {
    return u64(left) * u64(right);
}

fn field_multiply(left: FieldElement, right: FieldElement) -> FieldElement {
    let x = left.limbs;
    let y = right.limbs;
    let y1_19 = 19u * y[1];
    let y2_19 = 19u * y[2];
    let y3_19 = 19u * y[3];
    let y4_19 = 19u * y[4];
    let y5_19 = 19u * y[5];
    let y6_19 = 19u * y[6];
    let y7_19 = 19u * y[7];
    let y8_19 = 19u * y[8];
    let y9_19 = 19u * y[9];
    let x1_2 = 2u * x[1];
    let x3_2 = 2u * x[3];
    let x5_2 = 2u * x[5];
    let x7_2 = 2u * x[7];
    let x9_2 = 2u * x[9];

    var z: array<u64, 10>;
    z[0] = multiply_term(x[0], y[0]) + multiply_term(x1_2, y9_19)
        + multiply_term(x[2], y8_19) + multiply_term(x3_2, y7_19)
        + multiply_term(x[4], y6_19) + multiply_term(x5_2, y5_19)
        + multiply_term(x[6], y4_19) + multiply_term(x7_2, y3_19)
        + multiply_term(x[8], y2_19) + multiply_term(x9_2, y1_19);
    z[1] = multiply_term(x[0], y[1]) + multiply_term(x[1], y[0])
        + multiply_term(x[2], y9_19) + multiply_term(x[3], y8_19)
        + multiply_term(x[4], y7_19) + multiply_term(x[5], y6_19)
        + multiply_term(x[6], y5_19) + multiply_term(x[7], y4_19)
        + multiply_term(x[8], y3_19) + multiply_term(x[9], y2_19);
    z[2] = multiply_term(x[0], y[2]) + multiply_term(x1_2, y[1])
        + multiply_term(x[2], y[0]) + multiply_term(x3_2, y9_19)
        + multiply_term(x[4], y8_19) + multiply_term(x5_2, y7_19)
        + multiply_term(x[6], y6_19) + multiply_term(x7_2, y5_19)
        + multiply_term(x[8], y4_19) + multiply_term(x9_2, y3_19);
    z[3] = multiply_term(x[0], y[3]) + multiply_term(x[1], y[2])
        + multiply_term(x[2], y[1]) + multiply_term(x[3], y[0])
        + multiply_term(x[4], y9_19) + multiply_term(x[5], y8_19)
        + multiply_term(x[6], y7_19) + multiply_term(x[7], y6_19)
        + multiply_term(x[8], y5_19) + multiply_term(x[9], y4_19);
    z[4] = multiply_term(x[0], y[4]) + multiply_term(x1_2, y[3])
        + multiply_term(x[2], y[2]) + multiply_term(x3_2, y[1])
        + multiply_term(x[4], y[0]) + multiply_term(x5_2, y9_19)
        + multiply_term(x[6], y8_19) + multiply_term(x7_2, y7_19)
        + multiply_term(x[8], y6_19) + multiply_term(x9_2, y5_19);
    z[5] = multiply_term(x[0], y[5]) + multiply_term(x[1], y[4])
        + multiply_term(x[2], y[3]) + multiply_term(x[3], y[2])
        + multiply_term(x[4], y[1]) + multiply_term(x[5], y[0])
        + multiply_term(x[6], y9_19) + multiply_term(x[7], y8_19)
        + multiply_term(x[8], y7_19) + multiply_term(x[9], y6_19);
    z[6] = multiply_term(x[0], y[6]) + multiply_term(x1_2, y[5])
        + multiply_term(x[2], y[4]) + multiply_term(x3_2, y[3])
        + multiply_term(x[4], y[2]) + multiply_term(x5_2, y[1])
        + multiply_term(x[6], y[0]) + multiply_term(x7_2, y9_19)
        + multiply_term(x[8], y8_19) + multiply_term(x9_2, y7_19);
    z[7] = multiply_term(x[0], y[7]) + multiply_term(x[1], y[6])
        + multiply_term(x[2], y[5]) + multiply_term(x[3], y[4])
        + multiply_term(x[4], y[3]) + multiply_term(x[5], y[2])
        + multiply_term(x[6], y[1]) + multiply_term(x[7], y[0])
        + multiply_term(x[8], y9_19) + multiply_term(x[9], y8_19);
    z[8] = multiply_term(x[0], y[8]) + multiply_term(x1_2, y[7])
        + multiply_term(x[2], y[6]) + multiply_term(x3_2, y[5])
        + multiply_term(x[4], y[4]) + multiply_term(x5_2, y[3])
        + multiply_term(x[6], y[2]) + multiply_term(x7_2, y[1])
        + multiply_term(x[8], y[0]) + multiply_term(x9_2, y9_19);
    z[9] = multiply_term(x[0], y[9]) + multiply_term(x[1], y[8])
        + multiply_term(x[2], y[7]) + multiply_term(x[3], y[6])
        + multiply_term(x[4], y[5]) + multiply_term(x[5], y[4])
        + multiply_term(x[6], y[3]) + multiply_term(x[7], y[2])
        + multiply_term(x[8], y[1]) + multiply_term(x[9], y[0]);
    return field_reduce(z);
}

fn field_square(value: FieldElement) -> FieldElement {
    return field_multiply(value, value);
}

fn canonical_words(value: FieldElement) -> array<u32, 8> {
    var coefficients: array<u64, 10>;
    for (var index = 0u; index < 10u; index += 1u) {
        coefficients[index] = u64(value.limbs[index]);
    }
    var reduced = field_reduce(coefficients).limbs;
    var quotient = (reduced[0] + 19u) >> 26u;
    quotient = (reduced[1] + quotient) >> 25u;
    quotient = (reduced[2] + quotient) >> 26u;
    quotient = (reduced[3] + quotient) >> 25u;
    quotient = (reduced[4] + quotient) >> 26u;
    quotient = (reduced[5] + quotient) >> 25u;
    quotient = (reduced[6] + quotient) >> 26u;
    quotient = (reduced[7] + quotient) >> 25u;
    quotient = (reduced[8] + quotient) >> 26u;
    quotient = (reduced[9] + quotient) >> 25u;

    reduced[0] += 19u * quotient;
    reduced[1] += reduced[0] >> 26u;
    reduced[0] &= 0x3ffffffu;
    reduced[2] += reduced[1] >> 25u;
    reduced[1] &= 0x1ffffffu;
    reduced[3] += reduced[2] >> 26u;
    reduced[2] &= 0x3ffffffu;
    reduced[4] += reduced[3] >> 25u;
    reduced[3] &= 0x1ffffffu;
    reduced[5] += reduced[4] >> 26u;
    reduced[4] &= 0x3ffffffu;
    reduced[6] += reduced[5] >> 25u;
    reduced[5] &= 0x1ffffffu;
    reduced[7] += reduced[6] >> 26u;
    reduced[6] &= 0x3ffffffu;
    reduced[8] += reduced[7] >> 25u;
    reduced[7] &= 0x1ffffffu;
    reduced[9] += reduced[8] >> 26u;
    reduced[8] &= 0x3ffffffu;
    reduced[9] &= 0x1ffffffu;

    return array<u32, 8>(
        reduced[0] | (reduced[1] << 26u),
        (reduced[1] >> 6u) | (reduced[2] << 19u),
        (reduced[2] >> 13u) | (reduced[3] << 13u),
        (reduced[3] >> 19u) | (reduced[4] << 6u),
        reduced[5] | (reduced[6] << 25u),
        (reduced[6] >> 7u) | (reduced[7] << 19u),
        (reduced[7] >> 13u) | (reduced[8] << 12u),
        (reduced[8] >> 20u) | (reduced[9] << 6u),
    );
}

fn field_equal(left: FieldElement, right: FieldElement) -> bool {
    let left_words = canonical_words(left);
    let right_words = canonical_words(right);
    var difference = 0u;
    for (var index = 0u; index < 8u; index += 1u) {
        difference |= left_words[index] ^ right_words[index];
    }
    return difference == 0u;
}

fn field_is_zero(value: FieldElement) -> bool {
    let words = canonical_words(value);
    var combined = 0u;
    for (var index = 0u; index < 8u; index += 1u) {
        combined |= words[index];
    }
    return combined == 0u;
}

fn field_pow2k(value: FieldElement, power: u32) -> FieldElement {
    var result = field_square(value);
    for (var index = 1u; index < power; index += 1u) {
        result = field_square(result);
    }
    return result;
}

fn field_invert(value: FieldElement) -> FieldElement {
    let value_2 = field_square(value);
    var value_9 = field_pow2k(value_2, 2u);
    value_9 = field_multiply(value, value_9);
    let value_11 = field_multiply(value_2, value_9);
    let value_31 = field_multiply(value_9, field_square(value_11));
    let value_10_bits = field_multiply(field_pow2k(value_31, 5u), value_31);
    let value_20_bits = field_multiply(field_pow2k(value_10_bits, 10u), value_10_bits);
    let value_40_bits = field_multiply(field_pow2k(value_20_bits, 20u), value_20_bits);
    let value_50_bits = field_multiply(field_pow2k(value_40_bits, 10u), value_10_bits);
    let value_100_bits = field_multiply(field_pow2k(value_50_bits, 50u), value_50_bits);
    let value_200_bits = field_multiply(field_pow2k(value_100_bits, 100u), value_100_bits);
    let value_250_bits = field_multiply(field_pow2k(value_200_bits, 50u), value_50_bits);
    return field_multiply(field_pow2k(value_250_bits, 5u), value_11);
}

fn field_sqrt_power(value: FieldElement) -> FieldElement {
    let value_2 = field_square(value);
    var value_9 = field_pow2k(value_2, 2u);
    value_9 = field_multiply(value, value_9);
    let value_11 = field_multiply(value_2, value_9);
    let value_31 = field_multiply(value_9, field_square(value_11));
    let value_10_bits = field_multiply(field_pow2k(value_31, 5u), value_31);
    let value_20_bits = field_multiply(field_pow2k(value_10_bits, 10u), value_10_bits);
    let value_40_bits = field_multiply(field_pow2k(value_20_bits, 20u), value_20_bits);
    let value_50_bits = field_multiply(field_pow2k(value_40_bits, 10u), value_10_bits);
    let value_100_bits = field_multiply(field_pow2k(value_50_bits, 50u), value_50_bits);
    let value_200_bits = field_multiply(field_pow2k(value_100_bits, 100u), value_100_bits);
    let value_250_bits = field_multiply(field_pow2k(value_200_bits, 50u), value_50_bits);
    let value_252_minus_3 = field_multiply(field_pow2k(value_250_bits, 2u), value);
    return field_multiply(value_252_minus_3, value);
}

fn field_from_words(words: array<u32, 8>) -> FieldElement {
    return FieldElement(array<u32, 10>(
        words[0] & 0x3ffffffu,
        ((words[0] >> 26u) | (words[1] << 6u)) & 0x1ffffffu,
        ((words[1] >> 19u) | (words[2] << 13u)) & 0x3ffffffu,
        ((words[2] >> 13u) | (words[3] << 19u)) & 0x1ffffffu,
        (words[3] >> 6u) & 0x3ffffffu,
        words[4] & 0x1ffffffu,
        ((words[4] >> 25u) | (words[5] << 7u)) & 0x3ffffffu,
        ((words[5] >> 19u) | (words[6] << 13u)) & 0x1ffffffu,
        ((words[6] >> 12u) | (words[7] << 20u)) & 0x3ffffffu,
        (words[7] >> 6u) & 0x1ffffffu,
    ));
}

fn decompress(compressed: CompressedPoint) -> Point {
    var words = compressed.words;
    let sign = words[7] >> 31u;
    words[7] &= 0x7fffffffu;
    let y = field_from_words(words);
    let y_squared = field_square(y);
    let numerator = field_subtract(y_squared, field_one());
    let denominator = field_add(field_multiply(EDWARDS_D, y_squared), field_one());
    let x_squared = field_multiply(numerator, field_invert(denominator));
    var x = field_sqrt_power(x_squared);
    if !field_equal(field_square(x), x_squared) {
        x = field_multiply(x, SQRT_MINUS_ONE);
    }
    if (canonical_words(x)[0] & 1u) != sign {
        x = field_negate(x);
    }
    return Point(x, y, field_one(), field_multiply(x, y));
}

fn add_points(left: Point, right: Point) -> Point {
    let a = field_multiply(
        field_subtract(left.y, left.x),
        field_subtract(right.y, right.x),
    );
    let b = field_multiply(
        field_add(left.y, left.x),
        field_add(right.y, right.x),
    );
    let c = field_multiply(EDWARDS_D2, field_multiply(left.t, right.t));
    let z_product = field_multiply(left.z, right.z);
    let d = field_add(z_product, z_product);
    let e = field_subtract(b, a);
    let f = field_subtract(d, c);
    let g = field_add(d, c);
    let h = field_add(b, a);
    return Point(
        field_multiply(e, f),
        field_multiply(g, h),
        field_multiply(f, g),
        field_multiply(e, h),
    );
}

fn matches_any(candidate: array<u32, 8>) -> bool {
    for (var pattern_index = 0u; pattern_index < config.pattern_count; pattern_index += 1u) {
        var difference = 0u;
        for (var word = 0u; word < 8u; word += 1u) {
            difference |= (candidate[word] & patterns[pattern_index].mask[word])
                ^ patterns[pattern_index].value[word];
        }
        if difference == 0u {
            return true;
        }
    }
    return false;
}

fn center_index(bank: u32, group_index: u32) -> u32 {
    if bank == 0u {
        return group_index;
    }
    let scratch_start = config.workgroup_count + OFFSET_PAIRS + 1u;
    return scratch_start + (bank - 1u) * config.workgroup_count + group_index;
}

fn candidate_offset(delta: u64) -> vec2<u32> {
    let base = u64(config.base_low) | (u64(config.base_high) << 32u);
    let offset = base + delta;
    return vec2<u32>(u32(offset & 0xfffffffflu), u32(offset >> 32u));
}

fn record_candidate(candidate: array<u32, 8>, delta: u64) {
    if matches_any(candidate) {
        let match_index = atomicAdd(&search_result.found, 1u);
        if match_index < RESULT_CAPACITY {
            search_result.offsets[match_index] = candidate_offset(delta);
        }
    }
}

@compute @workgroup_size(64)
fn initialize(@builtin(global_invocation_id) global: vec3<u32>) {
    let point_count = config.workgroup_count + OFFSET_PAIRS + 1u;
    if global.x < point_count {
        points[global.x] = decompress(compressed_points[global.x]);
    }
}

@compute @workgroup_size(128)
fn search(
    @builtin(workgroup_id) group: vec3<u32>,
    @builtin(local_invocation_id) local: vec3<u32>,
) {
    let dispatched_group = group.x;
    let local_index = local.x;
    if dispatched_group >= config.group_count {
        return;
    }
    let group_index = config.group_start + dispatched_group;

    if local_index == 0u {
        center_point = points[center_index(config.source_bank, group_index)];
    }
    workgroupBarrier();

    var offset_point = points[config.workgroup_count];
    if local_index < OFFSET_PAIRS {
        offset_point = points[config.workgroup_count + local_index];
    }
    let stride_point = points[config.workgroup_count + OFFSET_PAIRS];

    for (var round = 0u; round < config.rounds; round += 1u) {
        var plus_numerator = field_zero();
        var minus_numerator = field_zero();

        if local_index < OFFSET_PAIRS {
            let x1_y2 = field_multiply(center_point.x, offset_point.y);
            let y1_x2 = field_multiply(center_point.y, offset_point.x);
            plus_numerator = field_subtract(center_point.t, offset_point.t);
            minus_numerator = field_add(center_point.t, offset_point.t);
            product_tree[256u + local_index * 2u] = field_subtract(x1_y2, y1_x2);
            product_tree[257u + local_index * 2u] = field_add(x1_y2, y1_x2);
        } else {
            next_center_projective = add_points(center_point, stride_point);
            product_tree[510] = next_center_projective.z;
            product_tree[511] = field_one();
        }
        workgroupBarrier();

        var width = 128u;
        loop {
            if local_index < width {
                let node = width + local_index;
                product_tree[node] = field_multiply(
                    product_tree[node * 2u],
                    product_tree[node * 2u + 1u],
                );
            }
            workgroupBarrier();
            if width == 1u {
                break;
            }
            width /= 2u;
        }

        if local_index == 0u {
            batch_valid = select(1u, 0u, field_is_zero(product_tree[1]));
            if batch_valid == 0u {
                atomicStore(&search_result.arithmetic_error, 1u);
            } else {
                product_tree[1] = field_invert(product_tree[1]);
            }
        }
        workgroupBarrier();
        if batch_valid == 0u {
            break;
        }

        var level_start = 1u;
        loop {
            if local_index < level_start {
                let parent = level_start + local_index;
                let parent_inverse = product_tree[parent];
                let left_product = product_tree[parent * 2u];
                let right_product = product_tree[parent * 2u + 1u];
                product_tree[parent * 2u] = field_multiply(parent_inverse, right_product);
                product_tree[parent * 2u + 1u] =
                    field_multiply(parent_inverse, left_product);
            }
            workgroupBarrier();
            if level_start == 128u {
                break;
            }
            level_start *= 2u;
        }

        let block_delta =
            (u64(round) * u64(config.workgroup_count) + u64(group_index))
                * u64(CANDIDATES_PER_GROUP);
        if local_index < OFFSET_PAIRS {
            let plus_y = field_multiply(
                plus_numerator,
                product_tree[256u + local_index * 2u],
            );
            let minus_y = field_multiply(
                minus_numerator,
                product_tree[257u + local_index * 2u],
            );
            record_candidate(
                canonical_words(plus_y),
                block_delta + 128lu + u64(local_index),
            );
            record_candidate(
                canonical_words(minus_y),
                block_delta + 126lu - u64(local_index),
            );
        } else {
            record_candidate(canonical_words(center_point.y), block_delta + 127lu);
            let inverse_z = product_tree[510];
            center_point = Point(
                field_multiply(next_center_projective.x, inverse_z),
                field_multiply(next_center_projective.y, inverse_z),
                field_one(),
                field_multiply(next_center_projective.t, inverse_z),
            );
        }
        workgroupBarrier();
    }

    if local_index == 0u && batch_valid != 0u {
        points[center_index(config.destination_bank, group_index)] = center_point;
    }
}

@compute @workgroup_size(64)
fn commit(@builtin(global_invocation_id) global: vec3<u32>) {
    if global.x < config.group_count {
        let group_index = config.group_start + global.x;
        points[center_index(0u, group_index)] =
            points[center_index(config.source_bank, group_index)];
    }
}
