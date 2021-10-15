pub fn md5hash(p: &[u8]) -> u128 {
    u128::from_be_bytes(*md5::compute(p))
}
