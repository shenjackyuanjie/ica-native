use super::*;

#[test]
fn stereo_device_rate_is_resampled_to_bridge_pcm_contract() {
    let mut input = vec![0.0; 48_000];
    input[0] = -2.0;
    input[2] = 2.0;
    input[4] = f32::NAN;
    let bytes = encode(&input, 48_000).unwrap();
    assert_eq!(bytes.len(), 44 + 24_000 * 2);
    assert_eq!(&bytes[22..24], &1_u16.to_le_bytes());
    assert_eq!(&bytes[24..28], &24_000_u32.to_le_bytes());
    assert_eq!(&bytes[44..50], &[0, 128, 255, 127, 0, 0]);
}

#[test]
fn empty_too_short_too_long_and_invalid_rate_are_rejected_before_encoding() {
    assert!(encode(&[], 24_000).is_err());
    assert!(encode(&vec![0.0; 4799], 24_000).is_err());
    assert!(encode(&vec![0.0; 4800], 24_000).is_ok());
    assert!(encode(&vec![0.0; 24_000 * 60 + 1], 24_000).is_err());
    assert!(encode(&[0.0; 4800], 0).is_err());
}
