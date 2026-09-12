pub const SAMPLE_RATE: u32 = 16_000;
pub const CHANNELS: u16 = 1;
pub const BITS_PER_SAMPLE: u16 = 16;

pub fn encode_pcm16le(samples: &[i16]) -> Vec<u8> {
    let data_size = std::mem::size_of_val(samples) as u32;
    let byte_rate = SAMPLE_RATE * u32::from(CHANNELS) * u32::from(BITS_PER_SAMPLE) / 8;
    let block_align = CHANNELS * BITS_PER_SAMPLE / 8;

    let mut wav = Vec::with_capacity(44 + data_size as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_size).to_le_bytes());
    wav.extend_from_slice(b"WAVE");

    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&CHANNELS.to_le_bytes());
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&BITS_PER_SAMPLE.to_le_bytes());

    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());

    for sample in samples {
        wav.extend_from_slice(&sample.to_le_bytes());
    }

    wav
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_valid_empty_pcm_wav_header() {
        let wav = encode_pcm16le(&[]);

        assert_eq!(wav.len(), 44);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 36);
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(u32::from_le_bytes(wav[16..20].try_into().unwrap()), 16);
        assert_eq!(u16::from_le_bytes(wav[20..22].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 1);
        assert_eq!(
            u32::from_le_bytes(wav[24..28].try_into().unwrap()),
            SAMPLE_RATE
        );
        assert_eq!(u32::from_le_bytes(wav[28..32].try_into().unwrap()), 32_000);
        assert_eq!(u16::from_le_bytes(wav[32..34].try_into().unwrap()), 2);
        assert_eq!(u16::from_le_bytes(wav[34..36].try_into().unwrap()), 16);
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 0);
    }

    #[test]
    fn appends_little_endian_samples_after_the_header() {
        let wav = encode_pcm16le(&[i16::MIN, -1, 0, 1, i16::MAX]);

        assert_eq!(wav.len(), 44 + 10);
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 46);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 10);
        assert_eq!(&wav[44..46], &i16::MIN.to_le_bytes());
        assert_eq!(&wav[46..48], &(-1_i16).to_le_bytes());
        assert_eq!(&wav[48..50], &0_i16.to_le_bytes());
        assert_eq!(&wav[50..52], &1_i16.to_le_bytes());
        assert_eq!(&wav[52..54], &i16::MAX.to_le_bytes());
    }
}
