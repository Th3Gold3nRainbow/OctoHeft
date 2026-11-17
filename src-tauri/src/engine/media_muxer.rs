use crate::engine::chunk_worker::send_request_with_headers;
use crate::engine::types::{SegmentProgress, SegmentStatus};
use bytes::Bytes;
use futures_util::StreamExt;
use mp4::{
    AacConfig, AudioObjectType, AvcConfig, ChannelConfig, HevcConfig, MediaConfig, Mp4Config,
    Mp4Reader, Mp4Sample, Mp4Writer, SampleFreqIndex, TrackConfig, TrackType, Vp9Config,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaTrackInfo {
    pub track_type: String, // "video" | "audio"
    pub codec: String,      // "avc1" | "mp4a" | etc.
    pub resolution: Option<String>,
    pub bandwidth: u64,
    pub init_url: Option<String>,
    pub segment_urls: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaManifestInfo {
    pub protocol: String, // "HLS" | "DASH"
    pub video_tracks: Vec<MediaTrackInfo>,
    pub audio_tracks: Vec<MediaTrackInfo>,
    pub total_duration_secs: f64,
    pub recommended_video_resolution: Option<String>,
    pub recommended_audio_bitrate: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MuxProgress {
    pub phase: String, // "parsing" | "downloading" | "demuxing" | "muxing" | "completed" | "error"
    pub segments_done: usize,
    pub segments_total: usize,
    pub percent: f64,
    #[serde(default)]
    pub downloaded_bytes: u64,
    #[serde(default)]
    pub total_bytes: u64,
    #[serde(default)]
    pub speed_bytes_per_sec: u64,
    #[serde(default)]
    pub eta_seconds: Option<u64>,
    #[serde(default)]
    pub active_segments: Vec<SegmentProgress>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackCodec {
    Avc,
    Hevc,
    Vp9,
    Vp8,
    Av1,
    Aac,
    Opus,
    Vorbis,
    Subtitle,
}

impl TrackCodec {
    pub fn as_display_name(self) -> &'static str {
        match self {
            TrackCodec::Avc => "H.264 / AVC",
            TrackCodec::Hevc => "H.265 / HEVC",
            TrackCodec::Vp9 => "VP9",
            TrackCodec::Vp8 => "VP8",
            TrackCodec::Av1 => "AV1",
            TrackCodec::Aac => "AAC",
            TrackCodec::Opus => "Opus",
            TrackCodec::Vorbis => "Vorbis",
            TrackCodec::Subtitle => "Subtitle",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputContainer {
    Mp4,
    WebM,
    Matroska,
}

impl OutputContainer {
    pub fn extension(self) -> &'static str {
        match self {
            OutputContainer::Mp4 => "mp4",
            OutputContainer::WebM => "webm",
            OutputContainer::Matroska => "mkv",
        }
    }

    pub fn ebml_doc_type(self) -> &'static str {
        match self {
            OutputContainer::WebM => "webm",
            _ => "matroska",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ParsedSample {
    pub start_time: u64,
    pub duration: u32,
    pub rendering_offset: i32,
    pub is_sync: bool,
    pub bytes: Bytes,
}

#[derive(Debug, Clone)]
pub struct ParsedIsoTrack {
    pub track_id: u32,
    pub timescale: u32,
    pub language: String,
    pub track_type: TrackType,
    pub codec: TrackCodec,
    pub media_conf: MediaConfig,
    pub width: u16,
    pub height: u16,
    pub display_width: u16,
    pub display_height: u16,
    pub sampling_frequency: f64,
    pub channels: u16,
    pub bit_depth: Option<u16>,
    pub codec_private: Option<Vec<u8>>,
    pub codec_delay_ns: u64,
    pub seek_pre_roll_ns: u64,
    pub default_duration_ns: u64,
    pub samples: Vec<ParsedSample>,
}

#[derive(Debug, Clone, Default)]
struct ParsedStsdEntry {
    track_type: Option<TrackType>,
    codec: Option<TrackCodec>,
    media_conf: Option<MediaConfig>,
    width: u16,
    height: u16,
    sampling_frequency: f64,
    channels: u16,
    bit_depth: Option<u16>,
    codec_private: Option<Vec<u8>>,
    codec_delay_ns: u64,
    seek_pre_roll_ns: u64,
}

#[derive(Debug, Clone, Default)]
struct TrakMetadata {
    track_id: u32,
    timescale: u32,
    language: String,
    track_type: Option<TrackType>,
    codec: Option<TrackCodec>,
    media_conf: Option<MediaConfig>,
    width: u16,
    height: u16,
    sampling_frequency: f64,
    channels: u16,
    bit_depth: Option<u16>,
    codec_private: Option<Vec<u8>>,
    codec_delay_ns: u64,
    seek_pre_roll_ns: u64,
    stbl_payload: Option<(usize, usize)>,
}

#[derive(Debug, Clone, Default)]
struct TrexDefaults {
    track_id: u32,
    default_sample_duration: u32,
    default_sample_size: u32,
    default_sample_flags: u32,
}

#[derive(Debug, Clone, Default)]
struct TfhdBox {
    track_id: u32,
    base_data_offset: Option<u64>,
    default_base_is_moof: bool,
    default_sample_duration: Option<u32>,
    default_sample_size: Option<u32>,
    default_sample_flags: Option<u32>,
}

#[allow(dead_code)]
struct IsoBox<'a> {
    offset: usize,
    total_len: usize,
    fourcc: [u8; 4],
    payload: &'a [u8],
}

fn iter_boxes_checked<'a>(
    data: &'a [u8],
    base_offset: usize,
    strict_eof: bool,
) -> Result<Vec<IsoBox<'a>>, String> {
    let mut boxes = Vec::new();
    let mut pos = 0usize;

    while pos < data.len() {
        if pos + 8 > data.len() {
            if strict_eof {
                return Err(format!(
                    "Truncated ISOBMFF box header at offset {} ({} trailing bytes)",
                    base_offset + pos,
                    data.len() - pos
                ));
            }
            break;
        }
        let size32 = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
        let fourcc = [data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]];

        let (header_len, total_len) = if size32 == 1 {
            if pos + 16 > data.len() {
                if strict_eof {
                    return Err(format!(
                        "Truncated 64-bit ISOBMFF box '{}' header at offset {}",
                        String::from_utf8_lossy(&fourcc),
                        base_offset + pos
                    ));
                }
                break;
            }
            let size64 = u64::from_be_bytes([
                data[pos + 8],
                data[pos + 9],
                data[pos + 10],
                data[pos + 11],
                data[pos + 12],
                data[pos + 13],
                data[pos + 14],
                data[pos + 15],
            ]);
            let total = usize::try_from(size64).map_err(|_| {
                format!(
                    "ISOBMFF box '{}' size overflow at offset {}",
                    String::from_utf8_lossy(&fourcc),
                    base_offset + pos
                )
            })?;
            (16usize, total)
        } else if size32 == 0 {
            (8usize, data.len() - pos)
        } else {
            (8usize, size32 as usize)
        };

        if total_len < header_len {
            if strict_eof {
                return Err(format!(
                    "Invalid ISOBMFF box '{}' size {} < header {} at offset {}",
                    String::from_utf8_lossy(&fourcc),
                    total_len,
                    header_len,
                    base_offset + pos
                ));
            }
            break;
        }
        if pos.saturating_add(total_len) > data.len() {
            if strict_eof || matches!(&fourcc, b"moov" | b"moof" | b"mdat" | b"traf" | b"trak" | b"stbl") {
                return Err(format!(
                    "Truncated ISOBMFF box '{}' at offset {}: declared {} bytes, remaining {} bytes",
                    String::from_utf8_lossy(&fourcc),
                    base_offset + pos,
                    total_len,
                    data.len() - pos
                ));
            }
            break;
        }

        boxes.push(IsoBox {
            offset: base_offset + pos,
            total_len,
            fourcc,
            payload: &data[pos + header_len..pos + total_len],
        });

        pos += total_len;
    }

    Ok(boxes)
}

fn iter_boxes<'a>(data: &'a [u8], base_offset: usize) -> Vec<IsoBox<'a>> {
    iter_boxes_checked(data, base_offset, false).unwrap_or_default()
}

fn read_u16_be(b: &[u8], off: usize) -> Option<u16> {
    b.get(off..off + 2).map(|s| u16::from_be_bytes([s[0], s[1]]))
}

fn read_u32_be(b: &[u8], off: usize) -> Option<u32> {
    b.get(off..off + 4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

fn read_i32_be(b: &[u8], off: usize) -> Option<i32> {
    b.get(off..off + 4)
        .map(|s| i32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

fn read_u64_be(b: &[u8], off: usize) -> Option<u64> {
    b.get(off..off + 8).map(|s| {
        u64::from_be_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]])
    })
}

fn decode_mdhd_language(packed: u16) -> String {
    let c1 = (((packed >> 10) & 0x1F) as u8).saturating_add(0x60);
    let c2 = (((packed >> 5) & 0x1F) as u8).saturating_add(0x60);
    let c3 = ((packed & 0x1F) as u8).saturating_add(0x60);
    if (0x61..=0x7a).contains(&c1)
        && (0x61..=0x7a).contains(&c2)
        && (0x61..=0x7a).contains(&c3)
    {
        String::from_utf8_lossy(&[c1, c2, c3]).to_string()
    } else {
        "und".to_string()
    }
}

fn parse_avcc(payload: &[u8], width: u16, height: u16) -> Option<AvcConfig> {
    if payload.len() < 8 {
        return None;
    }
    let num_sps = (payload[5] & 0x1F) as usize;
    let mut c = 6usize;
    let mut seq_param_set = Vec::new();
    for i in 0..num_sps {
        let sps_len = read_u16_be(payload, c)? as usize;
        c += 2;
        if c + sps_len > payload.len() {
            return None;
        }
        if i == 0 {
            seq_param_set = payload[c..c + sps_len].to_vec();
        }
        c += sps_len;
    }

    let num_pps = *payload.get(c)? as usize;
    c += 1;
    let mut pic_param_set = Vec::new();
    for i in 0..num_pps {
        let pps_len = read_u16_be(payload, c)? as usize;
        c += 2;
        if c + pps_len > payload.len() {
            return None;
        }
        if i == 0 {
            pic_param_set = payload[c..c + pps_len].to_vec();
        }
        c += pps_len;
    }

    if seq_param_set.is_empty() || pic_param_set.is_empty() {
        return None;
    }

    Some(AvcConfig {
        width: if width > 0 { width } else { 1920 },
        height: if height > 0 { height } else { 1080 },
        seq_param_set,
        pic_param_set,
    })
}

fn read_mp4_descr_len(data: &[u8], pos: &mut usize) -> Option<usize> {
    let mut len = 0usize;
    for _ in 0..4 {
        let b = *data.get(*pos)?;
        *pos += 1;
        len = (len << 7) | ((b & 0x7F) as usize);
        if (b & 0x80) == 0 {
            return Some(len);
        }
    }
    Some(len)
}

fn freq_index_from_hz(hz: u32) -> SampleFreqIndex {
    match hz {
        96000 => SampleFreqIndex::Freq96000,
        88200 => SampleFreqIndex::Freq88200,
        64000 => SampleFreqIndex::Freq64000,
        48000 => SampleFreqIndex::Freq48000,
        44100 => SampleFreqIndex::Freq44100,
        32000 => SampleFreqIndex::Freq32000,
        24000 => SampleFreqIndex::Freq24000,
        22050 => SampleFreqIndex::Freq22050,
        16000 => SampleFreqIndex::Freq16000,
        12000 => SampleFreqIndex::Freq12000,
        11025 => SampleFreqIndex::Freq11025,
        8000 => SampleFreqIndex::Freq8000,
        7350 => SampleFreqIndex::Freq7350,
        _ => SampleFreqIndex::Freq48000,
    }
}

fn synthesize_aac_asc(cfg: &AacConfig) -> Vec<u8> {
    let aot = (cfg.profile as u16) & 0x1F;
    let freq = (cfg.freq_index as u16) & 0x0F;
    let chan = (cfg.chan_conf as u16) & 0x0F;
    let word = (aot << 11) | (freq << 7) | (chan << 3);
    word.to_be_bytes().to_vec()
}

fn channels_from_chan_conf(chan_conf: ChannelConfig) -> u16 {
    match chan_conf {
        ChannelConfig::Mono => 1,
        ChannelConfig::Stereo => 2,
        ChannelConfig::Three => 3,
        ChannelConfig::Four => 4,
        ChannelConfig::Five => 5,
        ChannelConfig::FiveOne => 6,
        ChannelConfig::SevenOne => 8,
    }
}

fn parse_aac_from_asc(asc: &[u8], bitrate: u32, fallback_hz: u32) -> AacConfig {
    if asc.len() < 2 {
        return AacConfig {
            bitrate,
            profile: AudioObjectType::AacLowComplexity,
            freq_index: freq_index_from_hz(fallback_hz),
            chan_conf: ChannelConfig::Stereo,
        };
    }

    let word = u16::from_be_bytes([asc[0], asc[1]]);
    let aot_raw = ((word >> 11) & 0x1F) as u8;
    let freq_idx_raw = ((word >> 7) & 0x0F) as u8;
    let chan_raw = ((word >> 3) & 0x0F) as u8;

    let profile =
        AudioObjectType::try_from(aot_raw).unwrap_or(AudioObjectType::AacLowComplexity);
    let freq_index = SampleFreqIndex::try_from(freq_idx_raw)
        .unwrap_or_else(|_| freq_index_from_hz(fallback_hz));
    let chan_conf = ChannelConfig::try_from(chan_raw).unwrap_or(ChannelConfig::Stereo);

    AacConfig {
        bitrate,
        profile,
        freq_index,
        chan_conf,
    }
}

fn parse_esds_with_asc(payload: &[u8], fallback_hz: u32) -> Option<(AacConfig, Vec<u8>)> {
    if payload.len() < 6 {
        return None;
    }
    // FullBox: skip version (1) + flags (3)
    let mut pos = 4usize;
    let mut bitrate = 128_000u32;

    if payload.get(pos) == Some(&0x03) {
        pos += 1;
        let _es_len = read_mp4_descr_len(payload, &mut pos)?;
        if pos + 3 > payload.len() {
            return None;
        }
        let flags = payload[pos + 2];
        pos += 3;
        if (flags & 0x80) != 0 {
            pos = pos.saturating_add(2);
        }
        if (flags & 0x40) != 0 {
            let url_len = *payload.get(pos)? as usize;
            pos = pos.saturating_add(1 + url_len);
        }
        if (flags & 0x20) != 0 {
            pos = pos.saturating_add(2);
        }

        while pos < payload.len() {
            let tag = payload[pos];
            pos += 1;
            let len = read_mp4_descr_len(payload, &mut pos)?;
            let end = (pos + len).min(payload.len());

            if tag == 0x04 && end - pos >= 13 {
                if let Some(avg) = read_u32_be(payload, pos + 9) {
                    if avg > 0 {
                        bitrate = avg;
                    }
                }
                let mut inner = pos + 13;
                while inner < end {
                    let sub_tag = payload[inner];
                    inner += 1;
                    let sub_len = read_mp4_descr_len(payload, &mut inner)?;
                    let sub_end = (inner + sub_len).min(end);
                    if sub_tag == 0x05 {
                        let raw_asc = payload[inner..sub_end].to_vec();
                        let cfg = parse_aac_from_asc(&raw_asc, bitrate, fallback_hz);
                        let asc = if raw_asc.len() >= 2 {
                            raw_asc
                        } else {
                            synthesize_aac_asc(&cfg)
                        };
                        return Some((cfg, asc));
                    }
                    inner = sub_end;
                }
            } else if tag == 0x05 {
                let raw_asc = payload[pos..end].to_vec();
                let cfg = parse_aac_from_asc(&raw_asc, bitrate, fallback_hz);
                let asc = if raw_asc.len() >= 2 {
                    raw_asc
                } else {
                    synthesize_aac_asc(&cfg)
                };
                return Some((cfg, asc));
            }
            pos = end;
        }
    }

    let cfg = AacConfig {
        bitrate,
        profile: AudioObjectType::AacLowComplexity,
        freq_index: freq_index_from_hz(fallback_hz),
        chan_conf: ChannelConfig::Stereo,
    };
    let asc = synthesize_aac_asc(&cfg);
    Some((cfg, asc))
}

fn parse_dops_to_opus_head(payload: &[u8]) -> Option<(Vec<u8>, u16, u64)> {
    // dOps: version(1), channels(1), pre_skip(2 BE), input_sample_rate(4 BE), output_gain(2 BE), channel_mapping_family(1)
    if payload.len() < 11 {
        return None;
    }
    let channels = payload[1].max(1);
    let pre_skip = u16::from_be_bytes([payload[2], payload[3]]);
    let sample_rate = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
    let output_gain = u16::from_be_bytes([payload[8], payload[9]]);
    let mapping_family = payload[10];

    let mut opus_head = Vec::with_capacity(19 + payload.len().saturating_sub(11));
    opus_head.extend_from_slice(b"OpusHead");
    opus_head.push(1); // RFC 7845 version
    opus_head.push(channels);
    opus_head.extend_from_slice(&pre_skip.to_le_bytes());
    opus_head.extend_from_slice(&sample_rate.to_le_bytes());
    opus_head.extend_from_slice(&output_gain.to_le_bytes());
    opus_head.push(mapping_family);
    if mapping_family != 0 && payload.len() > 11 {
        opus_head.extend_from_slice(&payload[11..]);
    }

    let delay_ns = if pre_skip > 0 {
        (pre_skip as u64).saturating_mul(1_000_000_000) / 48_000
    } else {
        6_500_000
    };
    Some((opus_head, channels as u16, delay_ns))
}

fn synthesize_default_opus_head(channels: u16, sample_rate: u32) -> Vec<u8> {
    let mut opus_head = Vec::with_capacity(19);
    opus_head.extend_from_slice(b"OpusHead");
    opus_head.push(1);
    opus_head.push((channels.max(1).min(255)) as u8);
    opus_head.extend_from_slice(&312u16.to_le_bytes()); // 6.5ms @ 48kHz
    opus_head.extend_from_slice(&sample_rate.to_le_bytes());
    opus_head.extend_from_slice(&0u16.to_le_bytes());
    opus_head.push(0);
    opus_head
}

fn parse_vpcc_to_webm_private(payload: &[u8]) -> Option<Vec<u8>> {
    // ISOBMFF vpcC FullBox: version(1), flags(3), profile(1), level(1), bitDepth(4b)+chroma(3b)+fullRange(1b)
    if payload.len() < 7 {
        return None;
    }
    let profile = payload[4];
    let level = payload[5];
    let packed = payload[6];
    let bit_depth = (packed >> 4) & 0x0F;
    let chroma = (packed >> 1) & 0x07;
    // Matroska VP9 CodecPrivate TLV features (ID 1=Profile, 2=Level, 3=BitDepth, 4=ChromaSubsampling)
    Some(vec![
        0x01, 0x01, profile,
        0x02, 0x01, level,
        0x03, 0x01, bit_depth,
        0x04, 0x01, chroma,
    ])
}

fn parse_stsd(payload: &[u8]) -> Option<ParsedStsdEntry> {
    if payload.len() < 8 {
        return None;
    }
    // FullBox (4) + entry_count (4)
    for entry in iter_boxes(&payload[8..], 0) {
        match &entry.fourcc {
            b"avc1" | b"avc3" => {
                if entry.payload.len() >= 78 {
                    let width = read_u16_be(entry.payload, 24).unwrap_or(1920);
                    let height = read_u16_be(entry.payload, 26).unwrap_or(1080);
                    for child in iter_boxes(&entry.payload[78..], 0) {
                        if &child.fourcc == b"avcC" {
                            if let Some(cfg) = parse_avcc(child.payload, width, height) {
                                return Some(ParsedStsdEntry {
                                    track_type: Some(TrackType::Video),
                                    codec: Some(TrackCodec::Avc),
                                    media_conf: Some(MediaConfig::AvcConfig(cfg)),
                                    width,
                                    height,
                                    codec_private: Some(child.payload.to_vec()),
                                    ..Default::default()
                                });
                            }
                        }
                    }
                }
            }
            b"hvc1" | b"hev1" => {
                if entry.payload.len() >= 78 {
                    let width = read_u16_be(entry.payload, 24).unwrap_or(1920);
                    let height = read_u16_be(entry.payload, 26).unwrap_or(1080);
                    let mut hvcc_private = None;
                    for child in iter_boxes(&entry.payload[78..], 0) {
                        if &child.fourcc == b"hvcC" {
                            hvcc_private = Some(child.payload.to_vec());
                            break;
                        }
                    }
                    return Some(ParsedStsdEntry {
                        track_type: Some(TrackType::Video),
                        codec: Some(TrackCodec::Hevc),
                        media_conf: Some(MediaConfig::HevcConfig(HevcConfig { width, height })),
                        width,
                        height,
                        codec_private: hvcc_private,
                        ..Default::default()
                    });
                }
            }
            b"vp09" | b"vp08" => {
                if entry.payload.len() >= 78 {
                    let width = read_u16_be(entry.payload, 24).unwrap_or(1920);
                    let height = read_u16_be(entry.payload, 26).unwrap_or(1080);
                    let mut codec_private = None;
                    for child in iter_boxes(&entry.payload[78..], 0) {
                        if &child.fourcc == b"vpcC" {
                            codec_private = parse_vpcc_to_webm_private(child.payload);
                            break;
                        }
                    }
                    return Some(ParsedStsdEntry {
                        track_type: Some(TrackType::Video),
                        codec: Some(if &entry.fourcc == b"vp08" {
                            TrackCodec::Vp8
                        } else {
                            TrackCodec::Vp9
                        }),
                        media_conf: Some(MediaConfig::Vp9Config(Vp9Config { width, height })),
                        width,
                        height,
                        codec_private,
                        ..Default::default()
                    });
                }
            }
            b"av01" => {
                if entry.payload.len() >= 78 {
                    let width = read_u16_be(entry.payload, 24).unwrap_or(1920);
                    let height = read_u16_be(entry.payload, 26).unwrap_or(1080);
                    let mut av1c_private = None;
                    for child in iter_boxes(&entry.payload[78..], 0) {
                        if &child.fourcc == b"av1C" {
                            av1c_private = Some(child.payload.to_vec());
                            break;
                        }
                    }
                    return Some(ParsedStsdEntry {
                        track_type: Some(TrackType::Video),
                        codec: Some(TrackCodec::Av1),
                        media_conf: Some(MediaConfig::Vp9Config(Vp9Config { width, height })),
                        width,
                        height,
                        codec_private: av1c_private,
                        ..Default::default()
                    });
                }
            }
            b"mp4a" => {
                if entry.payload.len() >= 28 {
                    let version = read_u16_be(entry.payload, 8).unwrap_or(0);
                    let raw_channels = read_u16_be(entry.payload, 16).unwrap_or(2).max(1);
                    let fallback_hz = read_u16_be(entry.payload, 24).unwrap_or(48000) as u32;
                    let child_offset = match version {
                        1 if entry.payload.len() >= 44 => 44,
                        2 if entry.payload.len() >= 64 => 64,
                        _ => 28,
                    };

                    for child in iter_boxes(&entry.payload[child_offset..], 0) {
                        if &child.fourcc == b"esds" {
                            if let Some((cfg, asc)) = parse_esds_with_asc(child.payload, fallback_hz)
                            {
                                return Some(ParsedStsdEntry {
                                    track_type: Some(TrackType::Audio),
                                    codec: Some(TrackCodec::Aac),
                                    sampling_frequency: cfg.freq_index.freq() as f64,
                                    channels: channels_from_chan_conf(cfg.chan_conf),
                                    bit_depth: Some(16),
                                    media_conf: Some(MediaConfig::AacConfig(cfg)),
                                    codec_private: Some(asc),
                                    ..Default::default()
                                });
                            }
                        }
                    }

                    // Wave / nested QuickTime esds fallback
                    if entry.payload.len() > 28 {
                        for child in iter_boxes(&entry.payload[28..], 0) {
                            if &child.fourcc == b"esds" {
                                if let Some((cfg, asc)) =
                                    parse_esds_with_asc(child.payload, fallback_hz)
                                {
                                    return Some(ParsedStsdEntry {
                                        track_type: Some(TrackType::Audio),
                                        codec: Some(TrackCodec::Aac),
                                        sampling_frequency: cfg.freq_index.freq() as f64,
                                        channels: channels_from_chan_conf(cfg.chan_conf),
                                        bit_depth: Some(16),
                                        media_conf: Some(MediaConfig::AacConfig(cfg)),
                                        codec_private: Some(asc),
                                        ..Default::default()
                                    });
                                }
                            }
                        }
                    }

                    let cfg = AacConfig {
                        bitrate: 128_000,
                        profile: AudioObjectType::AacLowComplexity,
                        freq_index: freq_index_from_hz(fallback_hz),
                        chan_conf: ChannelConfig::Stereo,
                    };
                    let asc = synthesize_aac_asc(&cfg);
                    return Some(ParsedStsdEntry {
                        track_type: Some(TrackType::Audio),
                        codec: Some(TrackCodec::Aac),
                        sampling_frequency: cfg.freq_index.freq() as f64,
                        channels: raw_channels,
                        bit_depth: Some(16),
                        media_conf: Some(MediaConfig::AacConfig(cfg)),
                        codec_private: Some(asc),
                        ..Default::default()
                    });
                }
            }
            b"Opus" | b"opus" => {
                if entry.payload.len() >= 28 {
                    let version = read_u16_be(entry.payload, 8).unwrap_or(0);
                    let raw_channels = read_u16_be(entry.payload, 16).unwrap_or(2).max(1);
                    let sample_rate = read_u16_be(entry.payload, 24).unwrap_or(48000).max(8000) as u32;
                    let child_offset = match version {
                        1 if entry.payload.len() >= 44 => 44,
                        2 if entry.payload.len() >= 64 => 64,
                        _ => 28,
                    };

                    let mut opus_head = None;
                    let mut channels = raw_channels;
                    let mut delay_ns = 6_500_000u64;
                    for child in iter_boxes(&entry.payload[child_offset..], 0) {
                        if &child.fourcc == b"dOps" {
                            if let Some((head, ch, del)) = parse_dops_to_opus_head(child.payload) {
                                opus_head = Some(head);
                                channels = ch;
                                delay_ns = del;
                                break;
                            }
                        }
                    }

                    let cfg = AacConfig {
                        bitrate: 160_000,
                        profile: AudioObjectType::AacLowComplexity,
                        freq_index: SampleFreqIndex::Freq48000,
                        chan_conf: if channels == 1 {
                            ChannelConfig::Mono
                        } else {
                            ChannelConfig::Stereo
                        },
                    };
                    return Some(ParsedStsdEntry {
                        track_type: Some(TrackType::Audio),
                        codec: Some(TrackCodec::Opus),
                        sampling_frequency: sample_rate as f64,
                        channels,
                        bit_depth: None,
                        media_conf: Some(MediaConfig::AacConfig(cfg)),
                        codec_private: Some(
                            opus_head.unwrap_or_else(|| synthesize_default_opus_head(channels, sample_rate)),
                        ),
                        codec_delay_ns: delay_ns,
                        seek_pre_roll_ns: 80_000_000,
                        ..Default::default()
                    });
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_trak(payload: &[u8], payload_base: usize) -> Option<TrakMetadata> {
    let mut meta = TrakMetadata {
        timescale: 1000,
        language: "und".to_string(),
        ..Default::default()
    };

    for bx in iter_boxes(payload, payload_base) {
        match &bx.fourcc {
            b"tkhd" => {
                if let Some(&ver) = bx.payload.first() {
                    meta.track_id = if ver == 1 {
                        read_u32_be(bx.payload, 20).unwrap_or(1)
                    } else {
                        read_u32_be(bx.payload, 12).unwrap_or(1)
                    };
                }
            }
            b"mdia" => {
                let mdia_data_start = bx.offset + (bx.total_len - bx.payload.len());
                for mb in iter_boxes(bx.payload, mdia_data_start) {
                    match &mb.fourcc {
                        b"mdhd" => {
                            if let Some(&ver) = mb.payload.first() {
                                if ver == 1 {
                                    if let Some(ts) = read_u32_be(mb.payload, 20) {
                                        if ts > 0 {
                                            meta.timescale = ts;
                                        }
                                    }
                                    if let Some(lang) = read_u16_be(mb.payload, 32) {
                                        meta.language = decode_mdhd_language(lang);
                                    }
                                } else {
                                    if let Some(ts) = read_u32_be(mb.payload, 12) {
                                        if ts > 0 {
                                            meta.timescale = ts;
                                        }
                                    }
                                    if let Some(lang) = read_u16_be(mb.payload, 20) {
                                        meta.language = decode_mdhd_language(lang);
                                    }
                                }
                            }
                        }
                        b"minf" => {
                            let minf_data_start = mb.offset + (mb.total_len - mb.payload.len());
                            for minfb in iter_boxes(mb.payload, minf_data_start) {
                                if &minfb.fourcc == b"stbl" {
                                    let stbl_data_start =
                                        minfb.offset + (minfb.total_len - minfb.payload.len());
                                    meta.stbl_payload =
                                        Some((stbl_data_start, stbl_data_start + minfb.payload.len()));
                                    for stblb in iter_boxes(minfb.payload, stbl_data_start) {
                                        if &stblb.fourcc == b"stsd" {
                                            if let Some(entry) = parse_stsd(stblb.payload) {
                                                meta.track_type = entry.track_type;
                                                meta.codec = entry.codec;
                                                meta.media_conf = entry.media_conf;
                                                meta.width = entry.width;
                                                meta.height = entry.height;
                                                meta.sampling_frequency = entry.sampling_frequency;
                                                meta.channels = entry.channels;
                                                meta.bit_depth = entry.bit_depth;
                                                meta.codec_private = entry.codec_private;
                                                meta.codec_delay_ns = entry.codec_delay_ns;
                                                meta.seek_pre_roll_ns = entry.seek_pre_roll_ns;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    if meta.media_conf.is_some() && meta.codec.is_some() {
        Some(meta)
    } else {
        None
    }
}

fn parse_mvex(payload: &[u8]) -> Vec<TrexDefaults> {
    let mut out = Vec::new();
    for bx in iter_boxes(payload, 0) {
        if &bx.fourcc == b"trex" && bx.payload.len() >= 24 {
            out.push(TrexDefaults {
                track_id: read_u32_be(bx.payload, 4).unwrap_or(1),
                default_sample_duration: read_u32_be(bx.payload, 12).unwrap_or(0),
                default_sample_size: read_u32_be(bx.payload, 16).unwrap_or(0),
                default_sample_flags: read_u32_be(bx.payload, 20).unwrap_or(0),
            });
        }
    }
    out
}

fn parse_tfhd(payload: &[u8]) -> Option<TfhdBox> {
    if payload.len() < 8 {
        return None;
    }
    let flags = u32::from_be_bytes([0, payload[1], payload[2], payload[3]]);
    let track_id = read_u32_be(payload, 4)?;
    let mut c = 8usize;

    let base_data_offset = if (flags & 0x000001) != 0 {
        let v = read_u64_be(payload, c)?;
        c += 8;
        Some(v)
    } else {
        None
    };

    if (flags & 0x000002) != 0 {
        c = c.saturating_add(4); // sample_description_index
    }

    let default_sample_duration = if (flags & 0x000008) != 0 {
        let v = read_u32_be(payload, c)?;
        c += 4;
        Some(v)
    } else {
        None
    };

    let default_sample_size = if (flags & 0x000010) != 0 {
        let v = read_u32_be(payload, c)?;
        c += 4;
        Some(v)
    } else {
        None
    };

    let default_sample_flags = if (flags & 0x000020) != 0 {
        let v = read_u32_be(payload, c)?;
        Some(v)
    } else {
        None
    };

    Some(TfhdBox {
        track_id,
        base_data_offset,
        default_base_is_moof: (flags & 0x020000) != 0,
        default_sample_duration,
        default_sample_size,
        default_sample_flags,
    })
}

fn parse_non_fragmented_stbl(
    data: &Bytes,
    trak: &TrakMetadata,
    stbl_start: usize,
    stbl_end: usize,
) -> Result<Vec<ParsedSample>, String> {
    let stbl_slice = &data[stbl_start..stbl_end];
    let boxes = iter_boxes_checked(stbl_slice, stbl_start, false)?;

    let mut stts_entries: Vec<(u32, u32)> = Vec::new();
    let mut ctts_entries: Vec<(u32, i32)> = Vec::new();
    let mut stsc_entries: Vec<(u32, u32)> = Vec::new();
    let mut chunk_offsets: Vec<u64> = Vec::new();
    let mut constant_sample_size = 0u32;
    let mut declared_sample_count = 0usize;
    let mut sample_sizes: Vec<u32> = Vec::new();
    let mut has_stss = false;
    let mut sync_set: HashSet<u32> = HashSet::new();

    for bx in boxes {
        let p = bx.payload;
        match &bx.fourcc {
            b"stts" => {
                if p.len() < 8 {
                    return Err("Truncated stts box in ISOBMFF".to_string());
                }
                let count = read_u32_be(p, 4).unwrap_or(0) as usize;
                let needed = 8usize.saturating_add(count.saturating_mul(8));
                if p.len() < needed {
                    return Err("Truncated stts entries in ISOBMFF".to_string());
                }
                for i in 0..count {
                    let off = 8 + i * 8;
                    let n = read_u32_be(p, off).unwrap_or(0);
                    let d = read_u32_be(p, off + 4).unwrap_or(1);
                    stts_entries.push((n, d));
                }
            }
            b"ctts" => {
                if p.len() < 8 {
                    return Err("Truncated ctts box in ISOBMFF".to_string());
                }
                let ver = p[0];
                let count = read_u32_be(p, 4).unwrap_or(0) as usize;
                let needed = 8usize.saturating_add(count.saturating_mul(8));
                if p.len() < needed {
                    return Err("Truncated ctts entries in ISOBMFF".to_string());
                }
                for i in 0..count {
                    let off = 8 + i * 8;
                    let n = read_u32_be(p, off).unwrap_or(0);
                    let ro = if ver == 0 {
                        read_u32_be(p, off + 4).unwrap_or(0) as i32
                    } else {
                        read_i32_be(p, off + 4).unwrap_or(0)
                    };
                    ctts_entries.push((n, ro));
                }
            }
            b"stsc" => {
                if p.len() < 8 {
                    return Err("Truncated stsc box in ISOBMFF".to_string());
                }
                let count = read_u32_be(p, 4).unwrap_or(0) as usize;
                let needed = 8usize.saturating_add(count.saturating_mul(12));
                if p.len() < needed {
                    return Err("Truncated stsc entries in ISOBMFF".to_string());
                }
                for i in 0..count {
                    let off = 8 + i * 12;
                    let first_chunk = read_u32_be(p, off).unwrap_or(1);
                    let spc = read_u32_be(p, off + 4).unwrap_or(0);
                    stsc_entries.push((first_chunk, spc));
                }
            }
            b"stsz" => {
                if p.len() < 12 {
                    return Err("Truncated stsz box in ISOBMFF".to_string());
                }
                constant_sample_size = read_u32_be(p, 4).unwrap_or(0);
                declared_sample_count = read_u32_be(p, 8).unwrap_or(0) as usize;
                if constant_sample_size == 0 {
                    let needed = 12usize.saturating_add(declared_sample_count.saturating_mul(4));
                    if p.len() < needed {
                        return Err("Truncated stsz sample table in ISOBMFF".to_string());
                    }
                    sample_sizes.reserve(declared_sample_count);
                    for i in 0..declared_sample_count {
                        sample_sizes.push(read_u32_be(p, 12 + i * 4).unwrap_or(0));
                    }
                }
            }
            b"stco" => {
                if p.len() < 8 {
                    return Err("Truncated stco box in ISOBMFF".to_string());
                }
                let count = read_u32_be(p, 4).unwrap_or(0) as usize;
                let needed = 8usize.saturating_add(count.saturating_mul(4));
                if p.len() < needed {
                    return Err("Truncated stco chunk offsets in ISOBMFF".to_string());
                }
                chunk_offsets.reserve(count);
                for i in 0..count {
                    chunk_offsets.push(read_u32_be(p, 8 + i * 4).unwrap_or(0) as u64);
                }
            }
            b"co64" => {
                if p.len() < 8 {
                    return Err("Truncated co64 box in ISOBMFF".to_string());
                }
                let count = read_u32_be(p, 4).unwrap_or(0) as usize;
                let needed = 8usize.saturating_add(count.saturating_mul(8));
                if p.len() < needed {
                    return Err("Truncated co64 chunk offsets in ISOBMFF".to_string());
                }
                chunk_offsets.reserve(count);
                for i in 0..count {
                    chunk_offsets.push(read_u64_be(p, 8 + i * 8).unwrap_or(0));
                }
            }
            b"stss" => {
                if p.len() < 8 {
                    return Err("Truncated stss box in ISOBMFF".to_string());
                }
                has_stss = true;
                let count = read_u32_be(p, 4).unwrap_or(0) as usize;
                let needed = 8usize.saturating_add(count.saturating_mul(4));
                if p.len() < needed {
                    return Err("Truncated stss sync table in ISOBMFF".to_string());
                }
                for i in 0..count {
                    if let Some(sid) = read_u32_be(p, 8 + i * 4) {
                        sync_set.insert(sid);
                    }
                }
            }
            _ => {}
        }
    }

    if declared_sample_count == 0 || chunk_offsets.is_empty() || stsc_entries.is_empty() {
        return Err("Non-fragmented ISOBMFF stbl missing sample/chunk tables".to_string());
    }

    // Flatten sample timestamps & composition offsets
    let mut timings: Vec<(u64, u32)> = Vec::with_capacity(declared_sample_count);
    let mut cur_dts = 0u64;
    for (count, delta) in stts_entries {
        let d = delta.max(1);
        for _ in 0..count {
            if timings.len() >= declared_sample_count {
                break;
            }
            timings.push((cur_dts, d));
            cur_dts += d as u64;
        }
    }
    while timings.len() < declared_sample_count {
        timings.push((cur_dts, 1));
        cur_dts += 1;
    }

    let mut cts_offsets: Vec<i32> = Vec::with_capacity(declared_sample_count);
    if !ctts_entries.is_empty() {
        for (count, ro) in ctts_entries {
            for _ in 0..count {
                if cts_offsets.len() >= declared_sample_count {
                    break;
                }
                cts_offsets.push(ro);
            }
        }
    }

    let is_audio = trak.track_type == Some(TrackType::Audio);
    let mut samples = Vec::with_capacity(declared_sample_count);
    let mut sample_idx = 0usize;
    let mut stsc_pos = 0usize;

    for chunk_num in 1..=(chunk_offsets.len() as u32) {
        while stsc_pos + 1 < stsc_entries.len() && chunk_num >= stsc_entries[stsc_pos + 1].0 {
            stsc_pos += 1;
        }
        let spc = stsc_entries[stsc_pos].1 as usize;
        let mut cursor = usize::try_from(chunk_offsets[(chunk_num - 1) as usize])
            .map_err(|_| "ISOBMFF chunk offset overflow".to_string())?;

        for _ in 0..spc {
            if sample_idx >= declared_sample_count {
                break;
            }
            let sz = if constant_sample_size > 0 {
                constant_sample_size as usize
            } else {
                *sample_sizes.get(sample_idx).unwrap_or(&0) as usize
            };
            let end = cursor
                .checked_add(sz)
                .ok_or_else(|| "ISOBMFF sample offset overflow".to_string())?;
            if end > data.len() {
                return Err(format!(
                    "Truncated non-fragmented ISOBMFF sample {}: needs bytes {}..{}, buffer len {}",
                    sample_idx + 1,
                    cursor,
                    end,
                    data.len()
                ));
            }

            let (start_time, duration) = timings[sample_idx];
            let rendering_offset = cts_offsets.get(sample_idx).copied().unwrap_or(0);
            let is_sync = if is_audio || !has_stss {
                true
            } else {
                sync_set.contains(&((sample_idx + 1) as u32))
            };

            samples.push(ParsedSample {
                start_time,
                duration,
                rendering_offset,
                is_sync,
                bytes: data.slice(cursor..end),
            });

            cursor = end;
            sample_idx += 1;
        }
    }

    if samples.len() != declared_sample_count {
        return Err(format!(
            "ISOBMFF stbl sample count mismatch: decoded {} vs declared {}",
            samples.len(),
            declared_sample_count
        ));
    }

    Ok(samples)
}

/// Pure-Rust zero-copy ISOBMFF parser supporting both fragmented MP4 (`moov`+`mvex`/`trex`+multiple `moof`/`traf`/`trun`+`mdat`)
/// and standard non-fragmented MP4 (`stbl` sample tables) across AVC/H.264, HEVC, VP9, AV1, AAC, and Opus.
pub fn parse_isobmff_samples_bytes(data: Bytes) -> Result<ParsedIsoTrack, String> {
    let top_boxes = iter_boxes_checked(&data, 0, true)?;
    if top_boxes.is_empty() {
        return Err("Input buffer does not contain valid ISOBMFF boxes".to_string());
    }

    let mut chosen_trak: Option<TrakMetadata> = None;
    let mut trex_list: Vec<TrexDefaults> = Vec::new();
    let mut moof_ranges: Vec<(usize, usize, usize)> = Vec::new(); // (moof_offset, payload_start, payload_end)

    for bx in &top_boxes {
        match &bx.fourcc {
            b"moov" => {
                if chosen_trak.is_none() {
                    let moov_payload_start = bx.offset + (bx.total_len - bx.payload.len());
                    for mb in iter_boxes_checked(bx.payload, moov_payload_start, true)? {
                        let mb_payload_start = mb.offset + (mb.total_len - mb.payload.len());
                        match &mb.fourcc {
                            b"trak" => {
                                if chosen_trak.is_none() {
                                    chosen_trak = parse_trak(mb.payload, mb_payload_start);
                                }
                            }
                            b"mvex" => {
                                trex_list.extend(parse_mvex(mb.payload));
                            }
                            _ => {}
                        }
                    }
                }
            }
            b"moof" => {
                let p_start = bx.offset + (bx.total_len - bx.payload.len());
                moof_ranges.push((bx.offset, p_start, p_start + bx.payload.len()));
            }
            _ => {}
        }
    }

    let trak = chosen_trak.ok_or_else(|| {
        "ISOBMFF moov/trak missing supported codec configuration (avcC/mp4a/hvc1/vp09/av01/Opus)"
            .to_string()
    })?;

    let track_type = trak
        .track_type
        .ok_or_else(|| "Unknown ISOBMFF track type".to_string())?;
    let codec = trak
        .codec
        .ok_or_else(|| "Unknown ISOBMFF track codec".to_string())?;
    let media_conf = trak
        .media_conf
        .clone()
        .ok_or_else(|| "Missing ISOBMFF codec config".to_string())?;

    if moof_ranges.is_empty() {
        let (stbl_start, stbl_end) = trak
            .stbl_payload
            .ok_or_else(|| "Non-fragmented ISOBMFF track missing stbl sample table".to_string())?;
        let samples = parse_non_fragmented_stbl(&data, &trak, stbl_start, stbl_end)?;
        return Ok(ParsedIsoTrack {
            track_id: trak.track_id,
            timescale: trak.timescale.max(1),
            language: if trak.language.is_empty() {
                "und".to_string()
            } else {
                trak.language
            },
            track_type,
            codec,
            media_conf,
            width: trak.width,
            height: trak.height,
            display_width: trak.width,
            display_height: trak.height,
            sampling_frequency: trak.sampling_frequency,
            channels: trak.channels,
            bit_depth: trak.bit_depth,
            codec_private: trak.codec_private,
            codec_delay_ns: trak.codec_delay_ns,
            seek_pre_roll_ns: trak.seek_pre_roll_ns,
            default_duration_ns: 0,
            samples,
        });
    }

    let trex = trex_list
        .into_iter()
        .find(|t| t.track_id == trak.track_id)
        .unwrap_or_default();

    let mut samples = Vec::new();
    let mut running_dts = 0u64;
    let mut prev_traf_end: Option<u64> = None;

    for (moof_offset, p_start, p_end) in moof_ranges {
        let moof_payload = &data[p_start..p_end];
        for traf in iter_boxes_checked(moof_payload, p_start, true)? {
            if &traf.fourcc != b"traf" {
                continue;
            }

            let mut tfhd: Option<TfhdBox> = None;
            let mut tfdt: Option<u64> = None;
            let mut truns: Vec<&[u8]> = Vec::new();
            let traf_payload_start = traf.offset + (traf.total_len - traf.payload.len());

            for child in iter_boxes_checked(traf.payload, traf_payload_start, true)? {
                match &child.fourcc {
                    b"tfhd" => tfhd = parse_tfhd(child.payload),
                    b"tfdt" => {
                        if let Some(&ver) = child.payload.first() {
                            tfdt = if ver == 1 {
                                read_u64_be(child.payload, 4)
                            } else {
                                read_u32_be(child.payload, 4).map(|v| v as u64)
                            };
                        }
                    }
                    b"trun" => truns.push(child.payload),
                    _ => {}
                }
            }

            let Some(tfhd) = tfhd else {
                continue;
            };
            if tfhd.track_id != trak.track_id {
                continue;
            }
            if let Some(t) = tfdt {
                running_dts = t;
            }

            let base_offset = if let Some(bdo) = tfhd.base_data_offset {
                if bdo == 0 {
                    moof_offset as u64
                } else {
                    bdo
                }
            } else if tfhd.default_base_is_moof {
                moof_offset as u64
            } else {
                prev_traf_end.unwrap_or(moof_offset as u64)
            };

            let mut sample_cursor = usize::try_from(base_offset)
                .map_err(|_| "ISOBMFF base_data_offset overflow".to_string())?;

            for trun_payload in truns {
                if trun_payload.len() < 8 {
                    return Err("Truncated trun box header in fragmented ISOBMFF".to_string());
                }
                let ver = trun_payload[0];
                let flags = u32::from_be_bytes([0, trun_payload[1], trun_payload[2], trun_payload[3]]);
                let sample_count = read_u32_be(trun_payload, 4).unwrap_or(0) as usize;
                let mut c = 8usize;

                if (flags & 0x000001) != 0 {
                    let d_off = read_i32_be(trun_payload, c)
                        .ok_or_else(|| "Truncated trun data_offset".to_string())?;
                    let abs = (base_offset as i64) + (d_off as i64);
                    if abs < 0 {
                        return Err("Negative trun sample data offset".to_string());
                    }
                    sample_cursor = usize::try_from(abs)
                        .map_err(|_| "trun sample offset overflow".to_string())?;
                    c += 4;
                }

                let first_sample_flags = if (flags & 0x000004) != 0 {
                    let v = read_u32_be(trun_payload, c)
                        .ok_or_else(|| "Truncated trun first_sample_flags".to_string())?;
                    c += 4;
                    Some(v)
                } else {
                    None
                };

                for idx in 0..sample_count {
                    let duration = if (flags & 0x000100) != 0 {
                        let d = read_u32_be(trun_payload, c).ok_or_else(|| {
                            format!("Truncated trun duration at sample {}", idx)
                        })?;
                        c += 4;
                        d
                    } else {
                        tfhd.default_sample_duration
                            .unwrap_or(trex.default_sample_duration)
                            .max(1)
                    };

                    let size = if (flags & 0x000200) != 0 {
                        let s = read_u32_be(trun_payload, c).ok_or_else(|| {
                            format!("Truncated trun sample size at sample {}", idx)
                        })?;
                        c += 4;
                        s
                    } else {
                        tfhd.default_sample_size.unwrap_or(trex.default_sample_size)
                    } as usize;

                    let sample_flags = if (flags & 0x000400) != 0 {
                        let f = read_u32_be(trun_payload, c).ok_or_else(|| {
                            format!("Truncated trun sample flags at sample {}", idx)
                        })?;
                        c += 4;
                        f
                    } else if idx == 0 {
                        first_sample_flags
                            .or(tfhd.default_sample_flags)
                            .unwrap_or(trex.default_sample_flags)
                    } else {
                        tfhd.default_sample_flags.unwrap_or(trex.default_sample_flags)
                    };

                    let rendering_offset = if (flags & 0x000800) != 0 {
                        let val = if ver == 0 {
                            read_u32_be(trun_payload, c).map(|u| u as i32)
                        } else {
                            read_i32_be(trun_payload, c)
                        };
                        let ro = val.ok_or_else(|| {
                            format!("Truncated trun composition offset at sample {}", idx)
                        })?;
                        c += 4;
                        ro
                    } else {
                        0
                    };

                    let sample_end = sample_cursor
                        .checked_add(size)
                        .ok_or_else(|| "ISOBMFF sample cursor overflow".to_string())?;
                    if sample_end > data.len() {
                        return Err(format!(
                            "Truncated fragmented ISOBMFF stream: sample {} requires bytes {}..{}, buffer len {}",
                            idx,
                            sample_cursor,
                            sample_end,
                            data.len()
                        ));
                    }

                    let is_sync = if track_type == TrackType::Audio {
                        true
                    } else {
                        (sample_flags & 0x0001_0000) == 0
                    };

                    if size > 0 {
                        samples.push(ParsedSample {
                            start_time: running_dts,
                            duration,
                            rendering_offset,
                            is_sync,
                            bytes: data.slice(sample_cursor..sample_end),
                        });
                    }

                    sample_cursor = sample_end;
                    running_dts += duration as u64;
                }
            }

            prev_traf_end = Some(sample_cursor as u64);
        }
    }

    if samples.is_empty() {
        return Err("Fragmented MP4 contained 0 extractable media samples".to_string());
    }

    Ok(ParsedIsoTrack {
        track_id: trak.track_id,
        timescale: trak.timescale.max(1),
        language: if trak.language.is_empty() {
            "und".to_string()
        } else {
            trak.language
        },
        track_type,
        codec,
        media_conf,
        width: trak.width,
        height: trak.height,
        display_width: trak.width,
        display_height: trak.height,
        sampling_frequency: trak.sampling_frequency,
        channels: trak.channels,
        bit_depth: trak.bit_depth,
        codec_private: trak.codec_private,
        codec_delay_ns: trak.codec_delay_ns,
        seek_pre_roll_ns: trak.seek_pre_roll_ns,
        default_duration_ns: 0,
        samples,
    })
}

pub fn parse_isobmff_samples(data: &[u8]) -> Result<ParsedIsoTrack, String> {
    parse_isobmff_samples_bytes(Bytes::copy_from_slice(data))
}

fn concat_chunk_buffers(chunks: &[Option<Vec<u8>>]) -> Vec<u8> {
    let total: usize = chunks.iter().flatten().map(|v| v.len()).sum();
    let mut out = Vec::with_capacity(total);
    for c in chunks.iter().flatten() {
        out.extend_from_slice(c);
    }
    out
}

fn concat_owned_chunks(chunks: Vec<Vec<u8>>, total_hint: usize) -> Bytes {
    if chunks.len() == 1 {
        return Bytes::from(chunks.into_iter().next().unwrap());
    }
    let total: usize = chunks.iter().map(|c| c.len()).sum::<usize>().max(total_hint);
    let mut combined = Vec::with_capacity(total);
    for mut chunk in chunks {
        combined.append(&mut chunk);
        drop(chunk);
    }
    Bytes::from(combined)
}

fn read_ebml_id(data: &[u8], pos: usize) -> Option<(u32, usize)> {
    let first = *data.get(pos)?;
    if first == 0 {
        return None;
    }
    let lz = first.leading_zeros() as usize;
    let len = lz + 1;
    if len > 4 || pos + len > data.len() {
        return None;
    }
    let mut id = 0u32;
    for i in 0..len {
        id = (id << 8) | (data[pos + i] as u32);
    }
    Some((id, pos + len))
}

fn read_ebml_size(data: &[u8], pos: usize) -> Option<(Option<u64>, usize)> {
    let first = *data.get(pos)?;
    if first == 0 {
        return None;
    }
    let lz = first.leading_zeros() as usize;
    let len = lz + 1;
    if len > 8 || pos + len > data.len() {
        return None;
    }
    let mask = if lz >= 7 { 0 } else { (1u8 << (7 - lz)) - 1 };
    let mut val = (first & mask) as u64;
    for i in 1..len {
        val = (val << 8) | (data[pos + i] as u64);
    }
    let unknown_sentinel = if len * 7 >= 64 {
        u64::MAX
    } else {
        (1u64 << (len * 7)) - 1
    };
    if val == unknown_sentinel {
        Some((None, pos + len))
    } else {
        Some((Some(val), pos + len))
    }
}

fn read_ebml_vint_u64(data: &[u8], pos: usize) -> Option<(u64, usize, usize)> {
    let first = *data.get(pos)?;
    if first == 0 {
        return None;
    }
    let lz = first.leading_zeros() as usize;
    let len = lz + 1;
    if len > 8 || pos + len > data.len() {
        return None;
    }
    let mask = if lz >= 7 { 0 } else { (1u8 << (7 - lz)) - 1 };
    let mut val = (first & mask) as u64;
    for i in 1..len {
        val = (val << 8) | (data[pos + i] as u64);
    }
    Some((val, len, pos + len))
}

fn read_ebml_uint(slice: &[u8]) -> u64 {
    let mut v = 0u64;
    for &b in slice.iter().take(8) {
        v = (v << 8) | (b as u64);
    }
    v
}

fn read_ebml_float(slice: &[u8]) -> Option<f64> {
    match slice.len() {
        0 => Some(0.0),
        4 => Some(f32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]) as f64),
        8 => Some(f64::from_be_bytes([
            slice[0], slice[1], slice[2], slice[3], slice[4], slice[5], slice[6], slice[7],
        ])),
        _ => None,
    }
}

fn ebml_bounded_end(
    id: u32,
    p2: usize,
    size_opt: Option<u64>,
    limit: usize,
    ctx: &str,
) -> Result<usize, String> {
    match size_opt {
        Some(sz) => {
            let sz_usize = usize::try_from(sz).map_err(|_| {
                format!(
                    "EBML element 0x{:X} size overflow ({}) in {}",
                    id, sz, ctx
                )
            })?;
            let end = p2.checked_add(sz_usize).ok_or_else(|| {
                format!(
                    "EBML element 0x{:X} end offset overflow in {}",
                    id, ctx
                )
            })?;
            if end > limit {
                return Err(format!(
                    "Truncated EBML stream in {}: element 0x{:X} requires byte {}, available {}",
                    ctx, id, end, limit
                ));
            }
            Ok(end)
        }
        None => Ok(limit),
    }
}

#[derive(Debug, Default)]
struct WebmTrackMeta {
    track_number: u64,
    track_type: u64, // 1 = video, 2 = audio
    codec_id: String,
    language: String,
    width: u16,
    height: u16,
    display_width: u16,
    display_height: u16,
    sampling_frequency: f64,
    channels: u16,
    bit_depth: Option<u16>,
    codec_private: Option<Vec<u8>>,
    codec_delay_ns: u64,
    seek_pre_roll_ns: u64,
    default_duration_ns: u64,
}

fn parse_webm_track_entry(data: &[u8]) -> Result<WebmTrackMeta, String> {
    let mut meta = WebmTrackMeta {
        track_number: 1,
        language: "und".to_string(),
        ..Default::default()
    };
    let mut pos = 0usize;

    while pos < data.len() {
        let (id, p1) = read_ebml_id(data, pos)
            .ok_or_else(|| format!("Truncated EBML TrackEntry element ID at offset {}", pos))?;
        let (size_opt, p2) = read_ebml_size(data, p1).ok_or_else(|| {
            format!(
                "Truncated EBML TrackEntry size for 0x{:X} at offset {}",
                id, p1
            )
        })?;
        let end = ebml_bounded_end(id, p2, size_opt, data.len(), "TrackEntry")?;
        let body = &data[p2..end];
        match id {
            0xD7 => meta.track_number = read_ebml_uint(body),
            0x83 => meta.track_type = read_ebml_uint(body),
            0x86 => {
                meta.codec_id = String::from_utf8_lossy(body)
                    .trim_matches('\0')
                    .to_string();
            }
            0x22B59C => {
                let lang = String::from_utf8_lossy(body).trim_matches('\0').trim().to_string();
                if !lang.is_empty() {
                    meta.language = lang;
                }
            }
            0x23E383 => meta.default_duration_ns = read_ebml_uint(body),
            0x63A2 => meta.codec_private = Some(body.to_vec()),
            0x56AA => meta.codec_delay_ns = read_ebml_uint(body),
            0x56BB => meta.seek_pre_roll_ns = read_ebml_uint(body),
            0xE0 => {
                // Video settings container
                let mut vp = 0usize;
                while vp < body.len() {
                    let (vid, vp1) = read_ebml_id(body, vp)
                        .ok_or_else(|| "Truncated WebM Video element ID".to_string())?;
                    let (vsz, vp2) = read_ebml_size(body, vp1)
                        .ok_or_else(|| format!("Truncated WebM Video size for 0x{:X}", vid))?;
                    let vend = ebml_bounded_end(vid, vp2, vsz, body.len(), "VideoSettings")?;
                    let vbody = &body[vp2..vend];
                    match vid {
                        0xB0 => meta.width = read_ebml_uint(vbody) as u16,
                        0xBA => meta.height = read_ebml_uint(vbody) as u16,
                        0x54B0 => meta.display_width = read_ebml_uint(vbody) as u16,
                        0x54BA => meta.display_height = read_ebml_uint(vbody) as u16,
                        _ => {}
                    }
                    vp = vend.max(vp + 1);
                }
            }
            0xE1 => {
                // Audio settings container
                let mut ap = 0usize;
                while ap < body.len() {
                    let (aid, ap1) = read_ebml_id(body, ap)
                        .ok_or_else(|| "Truncated WebM Audio element ID".to_string())?;
                    let (asz, ap2) = read_ebml_size(body, ap1)
                        .ok_or_else(|| format!("Truncated WebM Audio size for 0x{:X}", aid))?;
                    let aend = ebml_bounded_end(aid, ap2, asz, body.len(), "AudioSettings")?;
                    let abody = &body[ap2..aend];
                    match aid {
                        0xB5 => {
                            if let Some(hz) = read_ebml_float(abody) {
                                meta.sampling_frequency = hz;
                            }
                        }
                        0x78B5 => {
                            if meta.sampling_frequency <= 0.0 {
                                if let Some(hz) = read_ebml_float(abody) {
                                    meta.sampling_frequency = hz;
                                }
                            }
                        }
                        0x9F => meta.channels = read_ebml_uint(abody) as u16,
                        0x6264 => meta.bit_depth = Some(read_ebml_uint(abody) as u16),
                        _ => {}
                    }
                    ap = aend.max(ap + 1);
                }
            }
            _ => {}
        }
        pos = end.max(pos + 1);
    }

    Ok(meta)
}

fn split_webm_block_frames_bytes(
    data: &Bytes,
    block_start: usize,
    block_end: usize,
) -> Result<(u64, i16, bool, Vec<Bytes>), String> {
    let block = &data[block_start..block_end];
    let (track_num, _, p1) = read_ebml_vint_u64(block, 0)
        .ok_or_else(|| "Truncated WebM block track number VINT".to_string())?;
    if p1 + 3 > block.len() {
        return Err("Truncated WebM block header (missing timecode/flags)".to_string());
    }
    let rel_tc = i16::from_be_bytes([block[p1], block[p1 + 1]]);
    let flags = block[p1 + 2];
    let is_key = (flags & 0x80) != 0;
    let lacing = (flags >> 1) & 0x03;
    let mut p = p1 + 3;

    let mut frames = Vec::new();
    match lacing {
        0 => {
            if p < block.len() {
                frames.push(data.slice(block_start + p..block_end));
            }
        }
        1 => {
            // Xiph lacing
            let count = (*block
                .get(p)
                .ok_or_else(|| "Truncated WebM Xiph lacing count".to_string())?
                as usize)
                + 1;
            p += 1;
            let mut sizes = Vec::with_capacity(count);
            for _ in 0..count.saturating_sub(1) {
                let mut sz = 0usize;
                loop {
                    let b = *block
                        .get(p)
                        .ok_or_else(|| "Truncated WebM Xiph lacing table".to_string())?;
                    p += 1;
                    sz += b as usize;
                    if b < 0xFF {
                        break;
                    }
                }
                sizes.push(sz);
            }
            for sz in sizes {
                let end_p = p
                    .checked_add(sz)
                    .ok_or_else(|| "WebM Xiph frame size overflow".to_string())?;
                if end_p > block.len() {
                    return Err("Truncated WebM Xiph laced frame".to_string());
                }
                frames.push(data.slice(block_start + p..block_start + end_p));
                p = end_p;
            }
            if p < block.len() {
                frames.push(data.slice(block_start + p..block_end));
            }
        }
        2 => {
            // Fixed-size lacing
            let count = (*block
                .get(p)
                .ok_or_else(|| "Truncated WebM fixed lacing count".to_string())?
                as usize)
                + 1;
            p += 1;
            let rem = block.len().saturating_sub(p);
            if count == 0 || rem % count != 0 {
                return Err("Invalid WebM fixed-size lacing frame count".to_string());
            }
            let each = rem / count;
            for _ in 0..count {
                frames.push(data.slice(block_start + p..block_start + p + each));
                p += each;
            }
        }
        _ => {
            // EBML lacing
            let count = (*block
                .get(p)
                .ok_or_else(|| "Truncated WebM EBML lacing count".to_string())?
                as usize)
                + 1;
            p += 1;
            let mut sizes = Vec::with_capacity(count);
            if count > 1 {
                let (first_sz, _, np) = read_ebml_vint_u64(block, p)
                    .ok_or_else(|| "Truncated WebM EBML first laced size".to_string())?;
                p = np;
                let mut prev = first_sz as i64;
                sizes.push(prev.max(0) as usize);
                for _ in 1..count.saturating_sub(1) {
                    let (raw_diff, len, np2) = read_ebml_vint_u64(block, p)
                        .ok_or_else(|| "Truncated WebM EBML laced delta".to_string())?;
                    p = np2;
                    let bias = ((1i64 << (7 * len - 1)) - 1) as i64;
                    prev += (raw_diff as i64) - bias;
                    if prev < 0 {
                        return Err("Negative WebM EBML laced frame size".to_string());
                    }
                    sizes.push(prev as usize);
                }
            }
            for sz in sizes {
                let end_p = p
                    .checked_add(sz)
                    .ok_or_else(|| "WebM EBML frame size overflow".to_string())?;
                if end_p > block.len() {
                    return Err("Truncated WebM EBML laced frame".to_string());
                }
                frames.push(data.slice(block_start + p..block_start + end_p));
                p = end_p;
            }
            if p < block.len() {
                frames.push(data.slice(block_start + p..block_end));
            }
        }
    }

    Ok((track_num, rel_tc, is_key, frames))
}

fn is_webm_segment_top_level(id: u32) -> bool {
    matches!(
        id,
        0x114D9B74 // SeekHead
        | 0x1549A966 // Info
        | 0x1654AE6B // Tracks
        | 0x1F43B675 // Cluster
        | 0x1C53BB6B // Cues
        | 0x1941A469 // Attachments
        | 0x1043A770 // Chapters
        | 0x1254C367 // Tags
    )
}

/// Pure-Rust zero-copy WebM / Matroska EBML parser supporting VP9, VP8, AV1, AVC, HEVC, Opus, Vorbis, and AAC tracks.
pub fn parse_webm_samples_bytes(data: Bytes) -> Result<ParsedIsoTrack, String> {
    if !data.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return Err("Buffer does not start with EBML header".to_string());
    }

    let mut pos = 0usize;
    let mut segment_range: Option<(usize, usize)> = None;

    while pos < data.len() {
        let (id, p1) = read_ebml_id(&data, pos)
            .ok_or_else(|| format!("Truncated top-level EBML element ID at offset {}", pos))?;
        let (size_opt, p2) = read_ebml_size(&data, p1).ok_or_else(|| {
            format!(
                "Truncated top-level EBML element size for 0x{:X} at offset {}",
                id, p1
            )
        })?;
        let end = ebml_bounded_end(id, p2, size_opt, data.len(), "EBML top-level")?;
        if id == 0x18538067 {
            segment_range = Some((p2, end));
            break;
        }
        pos = end.max(pos + 1);
    }

    let (seg_start, seg_end) =
        segment_range.ok_or_else(|| "WebM/Matroska stream missing EBML Segment".to_string())?;

    let mut timecode_scale_ns: u64 = 1_000_000; // default 1ms
    let mut parsed_tracks: Vec<WebmTrackMeta> = Vec::new();
    let mut selected_track: Option<WebmTrackMeta> = None;
    let mut raw_frames: Vec<(u64, u32, bool, Bytes)> = Vec::new();

    let mut sp = seg_start;
    while sp < seg_end {
        let (id, p1) = read_ebml_id(&data, sp)
            .ok_or_else(|| format!("Truncated Segment child ID at offset {}", sp))?;
        let (size_opt, p2) = read_ebml_size(&data, p1)
            .ok_or_else(|| format!("Truncated Segment child size for 0x{:X} at offset {}", id, p1))?;
        let child_end = ebml_bounded_end(id, p2, size_opt, seg_end, "Segment")?;

        match id {
            0x1549A966 => {
                // Segment Info
                let mut ip = p2;
                while ip < child_end {
                    let (iid, ip1) = read_ebml_id(&data, ip)
                        .ok_or_else(|| "Truncated Info child ID".to_string())?;
                    let (isz, ip2) = read_ebml_size(&data, ip1)
                        .ok_or_else(|| "Truncated Info child size".to_string())?;
                    let iend = ebml_bounded_end(iid, ip2, isz, child_end, "Info")?;
                    if iid == 0x2AD7B1 {
                        let s = read_ebml_uint(&data[ip2..iend]);
                        if s > 0 {
                            timecode_scale_ns = s;
                        }
                    }
                    ip = iend.max(ip + 1);
                }
            }
            0x1654AE6B => {
                // Tracks
                let mut tp = p2;
                while tp < child_end {
                    let (tid, tp1) = read_ebml_id(&data, tp)
                        .ok_or_else(|| "Truncated Tracks child ID".to_string())?;
                    let (tsz, tp2) = read_ebml_size(&data, tp1)
                        .ok_or_else(|| "Truncated Tracks child size".to_string())?;
                    let tend = ebml_bounded_end(tid, tp2, tsz, child_end, "Tracks")?;
                    if tid == 0xAE {
                        parsed_tracks.push(parse_webm_track_entry(&data[tp2..tend])?);
                    }
                    tp = tend.max(tp + 1);
                }
                if selected_track.is_none() && !parsed_tracks.is_empty() {
                    if let Some(idx) = parsed_tracks.iter().position(|t| t.track_type == 1) {
                        selected_track = Some(parsed_tracks.remove(idx));
                    } else if let Some(idx) = parsed_tracks.iter().position(|t| t.track_type == 2) {
                        selected_track = Some(parsed_tracks.remove(idx));
                    }
                }
            }
            0x1F43B675 => {
                // Cluster
                let mut cluster_time: u64 = 0;
                let mut cp = p2;
                let mut jumped_next = false;
                let target_track_num = selected_track
                    .as_ref()
                    .map(|m| m.track_number)
                    .unwrap_or(1);

                while cp < child_end {
                    let (cid, cp1) = read_ebml_id(&data, cp)
                        .ok_or_else(|| format!("Truncated Cluster child ID at offset {}", cp))?;
                    if is_webm_segment_top_level(cid) && size_opt.is_none() {
                        sp = cp;
                        jumped_next = true;
                        break;
                    }
                    let (csz, cp2) = read_ebml_size(&data, cp1)
                        .ok_or_else(|| format!("Truncated Cluster child size at offset {}", cp1))?;
                    let cend = ebml_bounded_end(cid, cp2, csz, child_end, "Cluster")?;

                    match cid {
                        0xE7 => {
                            cluster_time = read_ebml_uint(&data[cp2..cend]);
                        }
                        0xA3 => {
                            // SimpleBlock
                            let (tnum, rel, is_key, frames) =
                                split_webm_block_frames_bytes(&data, cp2, cend)?;
                            if tnum == target_track_num {
                                let abs_tc = ((cluster_time as i64) + (rel as i64)).max(0) as u64;
                                for frame in frames {
                                    raw_frames.push((abs_tc, 0, is_key, frame));
                                }
                            }
                        }
                        0xA0 => {
                            // BlockGroup
                            let mut bp = cp2;
                            let mut block_info: Option<(u64, i16, bool, Vec<Bytes>)> = None;
                            let mut block_dur: u32 = 0;
                            let mut has_ref = false;
                            while bp < cend {
                                let (bid, bp1) = read_ebml_id(&data, bp)
                                    .ok_or_else(|| "Truncated BlockGroup child ID".to_string())?;
                                let (bsz, bp2) = read_ebml_size(&data, bp1)
                                    .ok_or_else(|| "Truncated BlockGroup child size".to_string())?;
                                let bend = ebml_bounded_end(bid, bp2, bsz, cend, "BlockGroup")?;
                                match bid {
                                    0xA1 => {
                                        block_info =
                                            Some(split_webm_block_frames_bytes(&data, bp2, bend)?);
                                    }
                                    0x9B => {
                                        block_dur = read_ebml_uint(&data[bp2..bend]) as u32;
                                    }
                                    0xFB => {
                                        has_ref = true;
                                    }
                                    _ => {}
                                }
                                bp = bend.max(bp + 1);
                            }
                            if let Some((tnum, rel, _, frames)) = block_info {
                                if tnum == target_track_num {
                                    let abs_tc =
                                        ((cluster_time as i64) + (rel as i64)).max(0) as u64;
                                    let is_sync = !has_ref;
                                    for frame in frames {
                                        raw_frames.push((abs_tc, block_dur, is_sync, frame));
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                    cp = cend.max(cp + 1);
                }
                if !jumped_next {
                    sp = child_end.max(sp + 1);
                }
                continue;
            }
            _ => {}
        }

        sp = child_end.max(sp + 1);
    }

    let meta = selected_track
        .ok_or_else(|| "WebM/Matroska stream contains no supported video or audio track".to_string())?;
    let codec_upper = meta.codec_id.to_ascii_uppercase();

    let (track_type, codec) = if meta.track_type == 1 {
        let c = if codec_upper.is_empty() || codec_upper.starts_with("V_VP9") {
            TrackCodec::Vp9
        } else if codec_upper.starts_with("V_VP8") {
            TrackCodec::Vp8
        } else if codec_upper.starts_with("V_AV1") {
            TrackCodec::Av1
        } else if codec_upper.starts_with("V_MPEG4/ISO/AVC") {
            TrackCodec::Avc
        } else if codec_upper.starts_with("V_MPEGH/ISO/HEVC") {
            TrackCodec::Hevc
        } else {
            return Err(format!(
                "Unsupported Matroska/WebM video codec '{}'",
                meta.codec_id
            ));
        };
        (TrackType::Video, c)
    } else if meta.track_type == 2 {
        let c = if codec_upper.starts_with("A_OPUS") {
            TrackCodec::Opus
        } else if codec_upper.starts_with("A_VORBIS") {
            TrackCodec::Vorbis
        } else if codec_upper.starts_with("A_AAC") {
            TrackCodec::Aac
        } else {
            return Err(format!(
                "Unsupported Matroska/WebM audio codec '{}'",
                meta.codec_id
            ));
        };
        (TrackType::Audio, c)
    } else {
        return Err(format!("Unsupported WebM track type {}", meta.track_type));
    };

    if raw_frames.is_empty() {
        return Err(format!(
            "WebM/Matroska {} stream contained 0 media frames",
            codec.as_display_name()
        ));
    }

    let exact_ns_div = 1_000_000_000u64 % timecode_scale_ns.max(1) == 0
        && timecode_scale_ns <= 1_000_000
        && (1_000_000_000u64 / timecode_scale_ns.max(1)) <= u32::MAX as u64;

    let timescale: u32 = if exact_ns_div {
        (1_000_000_000u64 / timecode_scale_ns.max(1)) as u32
    } else {
        1_000_000
    };

    let scale_tick = |raw_tc: u64| -> u64 {
        if exact_ns_div {
            raw_tc
        } else {
            ((raw_tc as u128).saturating_mul(timecode_scale_ns as u128) / 1000) as u64
        }
    };

    let default_dur_ticks = if meta.default_duration_ns > 0 {
        ((meta.default_duration_ns as u128) * (timescale as u128) / 1_000_000_000u128)
            .max(1) as u32
    } else if track_type == TrackType::Audio {
        // Default 20ms frame (Opus) at track timescale
        (timescale / 50).max(1)
    } else {
        (timescale / 30).max(1)
    };

    raw_frames.sort_by_key(|f| f.0);

    let mut samples = Vec::with_capacity(raw_frames.len());
    for i in 0..raw_frames.len() {
        let (raw_ts, explicit_dur, is_sync, ref bytes) = raw_frames[i];
        let ts = scale_tick(raw_ts);
        let dur = if explicit_dur > 0 {
            let scaled = scale_tick(explicit_dur as u64);
            (scaled.min(u32::MAX as u64) as u32).max(1)
        } else if i + 1 < raw_frames.len() {
            let next_ts = scale_tick(raw_frames[i + 1].0);
            let delta = next_ts.saturating_sub(ts);
            if delta > 0 {
                (delta.min(u32::MAX as u64) as u32).max(1)
            } else {
                default_dur_ticks
            }
        } else {
            default_dur_ticks
        };
        samples.push(ParsedSample {
            start_time: ts,
            duration: dur,
            rendering_offset: 0,
            is_sync: if track_type == TrackType::Audio {
                true
            } else {
                is_sync
            },
            bytes: bytes.clone(),
        });
    }

    let width = if meta.width > 0 { meta.width } else { 1920 };
    let height = if meta.height > 0 { meta.height } else { 1080 };
    let display_width = if meta.display_width > 0 {
        meta.display_width
    } else {
        width
    };
    let display_height = if meta.display_height > 0 {
        meta.display_height
    } else {
        height
    };
    let channels = if meta.channels > 0 { meta.channels } else { 2 };
    let sampling_frequency = if meta.sampling_frequency > 0.0 {
        meta.sampling_frequency
    } else {
        48_000.0
    };

    let mut codec_private = meta.codec_private;
    let mut codec_delay_ns = meta.codec_delay_ns;
    let mut seek_pre_roll_ns = meta.seek_pre_roll_ns;

    if codec == TrackCodec::Opus {
        if codec_private.as_ref().map(|v| v.len() < 19).unwrap_or(true) {
            codec_private = Some(synthesize_default_opus_head(
                channels,
                sampling_frequency.round() as u32,
            ));
        }
        if codec_delay_ns == 0 {
            if let Some(ref head) = codec_private {
                if head.len() >= 12 && &head[..8] == b"OpusHead" {
                    let pre_skip = u16::from_le_bytes([head[10], head[11]]) as u64;
                    codec_delay_ns = if pre_skip > 0 {
                        pre_skip.saturating_mul(1_000_000_000) / 48_000
                    } else {
                        6_500_000
                    };
                } else {
                    codec_delay_ns = 6_500_000;
                }
            }
        }
        if seek_pre_roll_ns == 0 {
            seek_pre_roll_ns = 80_000_000;
        }
    }

    let media_conf = match codec {
        TrackCodec::Avc => MediaConfig::AvcConfig(
            codec_private
                .as_deref()
                .and_then(|cp| parse_avcc(cp, width, height))
                .unwrap_or_else(|| AvcConfig {
                    width,
                    height,
                    seq_param_set: Vec::new(),
                    pic_param_set: Vec::new(),
                }),
        ),
        TrackCodec::Hevc => MediaConfig::HevcConfig(HevcConfig { width, height }),
        TrackCodec::Vp9 | TrackCodec::Vp8 | TrackCodec::Av1 => {
            MediaConfig::Vp9Config(Vp9Config { width, height })
        }
        TrackCodec::Aac => {
            let hz = sampling_frequency.round() as u32;
            let cfg = codec_private
                .as_deref()
                .map(|asc| parse_aac_from_asc(asc, 128_000, hz))
                .unwrap_or_else(|| AacConfig {
                    bitrate: 128_000,
                    profile: AudioObjectType::AacLowComplexity,
                    freq_index: freq_index_from_hz(hz),
                    chan_conf: if channels == 1 {
                        ChannelConfig::Mono
                    } else {
                        ChannelConfig::Stereo
                    },
                });
            MediaConfig::AacConfig(cfg)
        }
        TrackCodec::Opus | TrackCodec::Vorbis | TrackCodec::Subtitle => {
            MediaConfig::AacConfig(AacConfig {
                bitrate: 160_000,
                profile: AudioObjectType::AacLowComplexity,
                freq_index: freq_index_from_hz(sampling_frequency.round() as u32),
                chan_conf: if channels == 1 {
                    ChannelConfig::Mono
                } else {
                    ChannelConfig::Stereo
                },
            })
        }
    };

    Ok(ParsedIsoTrack {
        track_id: 1,
        timescale,
        language: meta.language,
        track_type,
        codec,
        media_conf,
        width,
        height,
        display_width,
        display_height,
        sampling_frequency,
        channels,
        bit_depth: meta.bit_depth,
        codec_private,
        codec_delay_ns,
        seek_pre_roll_ns,
        default_duration_ns: meta.default_duration_ns,
        samples,
    })
}

pub fn parse_webm_vp9_samples(data: &[u8]) -> Result<ParsedIsoTrack, String> {
    parse_webm_samples_bytes(Bytes::copy_from_slice(data))
}

pub fn parse_media_samples_bytes(data: Bytes) -> Result<ParsedIsoTrack, String> {
    if data.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        parse_webm_samples_bytes(data)
    } else {
        parse_isobmff_samples_bytes(data)
    }
}

pub fn parse_media_samples(data: &[u8]) -> Result<ParsedIsoTrack, String> {
    parse_media_samples_bytes(Bytes::copy_from_slice(data))
}

pub fn select_output_container(
    video: &ParsedIsoTrack,
    audio: Option<&ParsedIsoTrack>,
) -> Result<OutputContainer, String> {
    let a_codec = audio.map(|a| a.codec);
    match (video.codec, a_codec) {
        (TrackCodec::Avc, None | Some(TrackCodec::Aac)) => Ok(OutputContainer::Mp4),
        (
            TrackCodec::Vp8 | TrackCodec::Vp9 | TrackCodec::Av1,
            None | Some(TrackCodec::Opus | TrackCodec::Vorbis),
        ) => Ok(OutputContainer::WebM),
        (
            TrackCodec::Vp8
            | TrackCodec::Vp9
            | TrackCodec::Av1
            | TrackCodec::Hevc
            | TrackCodec::Avc,
            None | Some(TrackCodec::Aac | TrackCodec::Opus | TrackCodec::Vorbis),
        ) => Ok(OutputContainer::Matroska),
        _ => Err(format!(
            "Unsupported codec pair for container muxing: {} + {:?}",
            video.codec.as_display_name(),
            a_codec.map(|c| c.as_display_name())
        )),
    }
}

pub fn resolve_output_path_for_container(
    destination_path: &Path,
    container: OutputContainer,
) -> PathBuf {
    let target_ext = container.extension();
    match destination_path.extension().and_then(|e| e.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case(target_ext) => destination_path.to_path_buf(),
        Some(ext)
            if matches!(
                ext.to_ascii_lowercase().as_str(),
                "mp4" | "webm" | "mkv" | "mov" | "m4v" | "ts" | "tmp" | "bin"
            ) =>
        {
            destination_path.with_extension(target_ext)
        }
        None => destination_path.with_extension(target_ext),
        Some(_) => {
            let name = destination_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("video");
            destination_path.with_file_name(format!("{}.{}", name, target_ext))
        }
    }
}

fn make_part_output_path(final_path: &Path) -> PathBuf {
    let file_name = final_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("output");
    final_path.with_file_name(format!("{}.octopart", file_name))
}

fn rename_part_to_final(tmp_path: &Path, final_path: &Path) -> Result<(), String> {
    if final_path.exists() {
        let _ = std::fs::remove_file(final_path);
    }
    std::fs::rename(tmp_path, final_path).map_err(|e| {
        format!(
            "Failed finalizing muxed container {} -> {}: {}",
            tmp_path.display(),
            final_path.display(),
            e
        )
    })
}

fn encode_ebml_vint_size(val: u64) -> Vec<u8> {
    if val < 0x7F {
        vec![0x80 | (val as u8)]
    } else if val < 0x3FFF {
        (0x4000u16 | (val as u16)).to_be_bytes().to_vec()
    } else if val < 0x1F_FFFF {
        let v = 0x20_0000u32 | (val as u32);
        vec![
            ((v >> 16) & 0xFF) as u8,
            ((v >> 8) & 0xFF) as u8,
            (v & 0xFF) as u8,
        ]
    } else if val < 0x0FFF_FFFF {
        (0x1000_0000u32 | (val as u32)).to_be_bytes().to_vec()
    } else if val < 0x07_FFFF_FFFF {
        let v = 0x08_0000_0000u64 | val;
        v.to_be_bytes()[3..8].to_vec()
    } else if val < 0x03FF_FFFF_FFFF {
        let v = 0x0400_0000_0000u64 | val;
        v.to_be_bytes()[2..8].to_vec()
    } else if val < 0x01_FFFF_FFFF_FFFF {
        let v = 0x02_0000_0000_0000u64 | val;
        v.to_be_bytes()[1..8].to_vec()
    } else {
        (0x0100_0000_0000_0000u64 | val).to_be_bytes().to_vec()
    }
}

fn encode_ebml_uint(val: u64) -> Vec<u8> {
    if val == 0 {
        return vec![0];
    }
    let be = val.to_be_bytes();
    let first_nz = be.iter().position(|&b| b != 0).unwrap_or(7);
    be[first_nz..].to_vec()
}

fn make_ebml(id: &[u8], body: &[u8]) -> Vec<u8> {
    let sz = encode_ebml_vint_size(body.len() as u64);
    let mut out = Vec::with_capacity(id.len() + sz.len() + body.len());
    out.extend_from_slice(id);
    out.extend_from_slice(&sz);
    out.extend_from_slice(body);
    out
}

fn build_seek_head_bytes(info_pos: u64, tracks_pos: u64, cues_pos: u64) -> Vec<u8> {
    let entries: [(&[u8; 4], u64); 3] = [
        (&[0x15, 0x49, 0xA9, 0x66], info_pos),
        (&[0x16, 0x54, 0xAE, 0x6B], tracks_pos),
        (&[0x1C, 0x53, 0xBB, 0x6B], cues_pos),
    ];
    let mut body = Vec::new();
    for (id_bytes, pos) in entries {
        let mut seek_body = Vec::new();
        seek_body.extend_from_slice(&make_ebml(&[0x53, 0xAB], id_bytes));
        // Fixed 8-byte SeekPosition so placeholder patching never changes byte offsets
        seek_body.extend_from_slice(&make_ebml(&[0x53, 0xAC], &pos.to_be_bytes()));
        body.extend_from_slice(&make_ebml(&[0x4D, 0xBB], &seek_body));
    }
    make_ebml(&[0x11, 0x4D, 0x9B, 0x74], &body)
}

fn build_avcc_from_config(cfg: &AvcConfig) -> Option<Vec<u8>> {
    if cfg.seq_param_set.len() < 4 || cfg.pic_param_set.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    out.push(1);
    out.push(cfg.seq_param_set[1]);
    out.push(cfg.seq_param_set[2]);
    out.push(cfg.seq_param_set[3]);
    out.push(0xFF);
    out.push(0xE1);
    out.extend_from_slice(&(cfg.seq_param_set.len() as u16).to_be_bytes());
    out.extend_from_slice(&cfg.seq_param_set);
    out.push(1);
    out.extend_from_slice(&(cfg.pic_param_set.len() as u16).to_be_bytes());
    out.extend_from_slice(&cfg.pic_param_set);
    Some(out)
}

fn verify_mp4_container_file(
    path: &Path,
    expected_video_samples: usize,
    expected_audio_samples: usize,
) -> Result<u64, String> {
    let file_len = std::fs::metadata(path)
        .map_err(|e| format!("Failed reading MP4 metadata: {}", e))?
        .len();
    if file_len == 0 {
        return Err("Verification failed: MP4 output file is 0 bytes".to_string());
    }
    let file = File::open(path).map_err(|e| format!("Failed opening MP4 for verification: {}", e))?;
    let mut reader = Mp4Reader::read_header(file, file_len)
        .map_err(|e| format!("Verification failed: invalid MP4 structure ({})", e))?;

    let expected_tracks = if expected_audio_samples > 0 { 2 } else { 1 };
    if reader.tracks().len() < expected_tracks {
        return Err(format!(
            "Verification failed: MP4 contains {} tracks, expected {}",
            reader.tracks().len(),
            expected_tracks
        ));
    }

    let v_count = reader
        .tracks()
        .get(&1)
        .map(|t| t.sample_count())
        .unwrap_or(0);
    if v_count as usize != expected_video_samples {
        return Err(format!(
            "Verification failed: MP4 track 1 has {} samples, expected {}",
            v_count, expected_video_samples
        ));
    }
    if v_count > 0 {
        if reader.read_sample(1, 1).map_err(|e| e.to_string())?.is_none()
            || reader
                .read_sample(1, v_count)
                .map_err(|e| e.to_string())?
                .is_none()
        {
            return Err("Verification failed: MP4 video samples unreadable".to_string());
        }
    }

    if expected_audio_samples > 0 {
        let a_count = reader
            .tracks()
            .get(&2)
            .map(|t| t.sample_count())
            .unwrap_or(0);
        if a_count as usize != expected_audio_samples {
            return Err(format!(
                "Verification failed: MP4 track 2 has {} samples, expected {}",
                a_count, expected_audio_samples
            ));
        }
        if a_count > 0 {
            if reader.read_sample(2, 1).map_err(|e| e.to_string())?.is_none()
                || reader
                    .read_sample(2, a_count)
                    .map_err(|e| e.to_string())?
                    .is_none()
            {
                return Err("Verification failed: MP4 audio samples unreadable".to_string());
            }
        }
    }

    Ok(file_len)
}

fn stream_read_ebml_id<R: Read>(r: &mut R) -> Result<(u32, u64), String> {
    let mut first = [0u8; 1];
    r.read_exact(&mut first)
        .map_err(|e| format!("Failed reading EBML ID: {}", e))?;
    if first[0] == 0 {
        return Err("Invalid zero EBML ID lead byte".to_string());
    }
    let len = (first[0].leading_zeros() as usize) + 1;
    if len > 4 {
        return Err("Invalid EBML ID length > 4".to_string());
    }
    let mut buf = [0u8; 4];
    buf[0] = first[0];
    if len > 1 {
        r.read_exact(&mut buf[1..len])
            .map_err(|e| format!("Truncated EBML ID bytes: {}", e))?;
    }
    let mut id = 0u32;
    for &b in &buf[..len] {
        id = (id << 8) | (b as u32);
    }
    Ok((id, len as u64))
}

fn stream_read_ebml_size<R: Read>(r: &mut R) -> Result<(Option<u64>, u64), String> {
    let mut first = [0u8; 1];
    r.read_exact(&mut first)
        .map_err(|e| format!("Failed reading EBML size: {}", e))?;
    if first[0] == 0 {
        return Err("Invalid zero EBML size lead byte".to_string());
    }
    let lz = first[0].leading_zeros() as usize;
    let len = lz + 1;
    if len > 8 {
        return Err("Invalid EBML size VINT > 8 bytes".to_string());
    }
    let mask = if lz >= 7 { 0 } else { (1u8 << (7 - lz)) - 1 };
    let mut val = (first[0] & mask) as u64;
    if len > 1 {
        let mut rest = [0u8; 7];
        r.read_exact(&mut rest[..len - 1])
            .map_err(|e| format!("Truncated EBML size bytes: {}", e))?;
        for &b in &rest[..len - 1] {
            val = (val << 8) | (b as u64);
        }
    }
    let unknown_sentinel = if len * 7 >= 64 {
        u64::MAX
    } else {
        (1u64 << (len * 7)) - 1
    };
    if val == unknown_sentinel {
        Ok((None, len as u64))
    } else {
        Ok((Some(val), len as u64))
    }
}

fn stream_read_vint_u64<R: Read>(r: &mut R) -> Result<(u64, u64), String> {
    let mut first = [0u8; 1];
    r.read_exact(&mut first)
        .map_err(|e| format!("Failed reading VINT: {}", e))?;
    if first[0] == 0 {
        return Err("Invalid zero VINT lead byte".to_string());
    }
    let lz = first[0].leading_zeros() as usize;
    let len = lz + 1;
    if len > 8 {
        return Err("Invalid VINT > 8 bytes".to_string());
    }
    let mask = if lz >= 7 { 0 } else { (1u8 << (7 - lz)) - 1 };
    let mut val = (first[0] & mask) as u64;
    if len > 1 {
        let mut rest = [0u8; 7];
        r.read_exact(&mut rest[..len - 1])
            .map_err(|e| format!("Truncated VINT bytes: {}", e))?;
        for &b in &rest[..len - 1] {
            val = (val << 8) | (b as u64);
        }
    }
    Ok((val, len as u64))
}

fn verify_ebml_container_file(
    path: &Path,
    expected_doc_type: &str,
    expected_video_samples: usize,
    expected_audio_samples: usize,
) -> Result<u64, String> {
    let file_len = std::fs::metadata(path)
        .map_err(|e| format!("Failed reading EBML metadata: {}", e))?
        .len();
    if file_len < 32 {
        return Err(format!("EBML output too small ({} bytes)", file_len));
    }

    let file = File::open(path).map_err(|e| format!("Failed opening EBML file for verification: {}", e))?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);

    let (ebml_id, id_len) = stream_read_ebml_id(&mut reader)?;
    if ebml_id != 0x1A45DFA3 {
        return Err(format!("Verification failed: invalid EBML header ID 0x{:X}", ebml_id));
    }
    let (ebml_sz_opt, sz_len) = stream_read_ebml_size(&mut reader)?;
    let ebml_sz = ebml_sz_opt.ok_or_else(|| "EBML header cannot have unknown size".to_string())?;
    if ebml_sz > 4096 || id_len + sz_len + ebml_sz >= file_len {
        return Err("Verification failed: invalid EBML header size".to_string());
    }
    let mut hdr_body = vec![0u8; ebml_sz as usize];
    reader
        .read_exact(&mut hdr_body)
        .map_err(|e| format!("Failed reading EBML header body: {}", e))?;

    let mut hp = 0usize;
    let mut doc_type = String::new();
    while hp < hdr_body.len() {
        let (hid, hp1) = read_ebml_id(&hdr_body, hp).ok_or_else(|| "Invalid EBML header sub-ID".to_string())?;
        let (hsz, hp2) = read_ebml_size(&hdr_body, hp1).ok_or_else(|| "Invalid EBML header sub-size".to_string())?;
        let hend = ebml_bounded_end(hid, hp2, hsz, hdr_body.len(), "VerifyEBMLHeader")?;
        if hid == 0x4282 {
            doc_type = String::from_utf8_lossy(&hdr_body[hp2..hend])
                .trim_matches('\0')
                .to_string();
        }
        hp = hend.max(hp + 1);
    }
    if doc_type != expected_doc_type {
        return Err(format!(
            "Verification failed: EBML DocType '{}' != expected '{}'",
            doc_type, expected_doc_type
        ));
    }

    let mut pos = id_len + sz_len + ebml_sz;
    reader
        .seek(SeekFrom::Start(pos))
        .map_err(|e| format!("Seek error before Segment: {}", e))?;

    let (seg_id, seg_id_len) = stream_read_ebml_id(&mut reader)?;
    if seg_id != 0x18538067 {
        return Err(format!("Verification failed: expected Segment ID, got 0x{:X}", seg_id));
    }
    let (seg_sz_opt, seg_sz_len) = stream_read_ebml_size(&mut reader)?;
    let seg_sz = seg_sz_opt
        .ok_or_else(|| "Verification failed: Segment size was unpatched unknown sentinel".to_string())?;
    let seg_data_start = pos + seg_id_len + seg_sz_len;
    let seg_end = seg_data_start + seg_sz;
    if seg_end != file_len {
        return Err(format!(
            "Verification failed: Segment end {} != file length {}",
            seg_end, file_len
        ));
    }

    let mut saw_seek_head = false;
    let mut saw_info = false;
    let mut info_valid = false;
    let mut saw_tracks = false;
    let mut tracks_count = 0usize;
    let mut cluster_count = 0usize;
    let mut saw_cues = false;
    let mut track1_blocks = 0usize;
    let mut track2_blocks = 0usize;

    pos = seg_data_start;
    while pos < seg_end {
        reader
            .seek(SeekFrom::Start(pos))
            .map_err(|e| format!("Seek error in Segment: {}", e))?;
        let (eid, eid_len) = stream_read_ebml_id(&mut reader)?;
        let (esz_opt, esz_len) = stream_read_ebml_size(&mut reader)?;
        let esz = esz_opt
            .ok_or_else(|| format!("Verification failed: unknown-size Segment child 0x{:X}", eid))?;
        let edata_start = pos + eid_len + esz_len;
        let eend = edata_start
            .checked_add(esz)
            .ok_or_else(|| "EBML element end overflow".to_string())?;
        if eend > seg_end {
            return Err(format!(
                "Verification failed: EBML element 0x{:X} extends past Segment end ({} > {})",
                eid, eend, seg_end
            ));
        }

        match eid {
            0x114D9B74 => saw_seek_head = true,
            0x1549A966 => {
                saw_info = true;
                if esz <= 64 * 1024 {
                    let mut buf = vec![0u8; esz as usize];
                    reader.read_exact(&mut buf).map_err(|e| e.to_string())?;
                    let mut ip = 0usize;
                    let mut scale = 0u64;
                    let mut dur = 0.0f64;
                    while ip < buf.len() {
                        let (iid, ip1) = read_ebml_id(&buf, ip).ok_or_else(|| "Invalid Info ID".to_string())?;
                        let (isz, ip2) = read_ebml_size(&buf, ip1).ok_or_else(|| "Invalid Info size".to_string())?;
                        let iend = ebml_bounded_end(iid, ip2, isz, buf.len(), "VerifyInfo")?;
                        if iid == 0x2AD7B1 {
                            scale = read_ebml_uint(&buf[ip2..iend]);
                        } else if iid == 0x4489 {
                            dur = read_ebml_float(&buf[ip2..iend]).unwrap_or(0.0);
                        }
                        ip = iend.max(ip + 1);
                    }
                    info_valid = scale > 0 && dur > 0.0;
                }
            }
            0x1654AE6B => {
                saw_tracks = true;
                if esz <= 256 * 1024 {
                    let mut buf = vec![0u8; esz as usize];
                    reader.read_exact(&mut buf).map_err(|e| e.to_string())?;
                    let mut tp = 0usize;
                    while tp < buf.len() {
                        let (tid, tp1) = read_ebml_id(&buf, tp).ok_or_else(|| "Invalid Tracks ID".to_string())?;
                        let (tsz, tp2) = read_ebml_size(&buf, tp1).ok_or_else(|| "Invalid Tracks size".to_string())?;
                        let tend = ebml_bounded_end(tid, tp2, tsz, buf.len(), "VerifyTracks")?;
                        if tid == 0xAE {
                            tracks_count += 1;
                        }
                        tp = tend.max(tp + 1);
                    }
                }
            }
            0x1F43B675 => {
                cluster_count += 1;
                let mut cp = edata_start;
                while cp < eend {
                    reader
                        .seek(SeekFrom::Start(cp))
                        .map_err(|e| format!("Cluster seek error: {}", e))?;
                    let (cid, cid_len) = stream_read_ebml_id(&mut reader)?;
                    let (csz_opt, csz_len) = stream_read_ebml_size(&mut reader)?;
                    let csz = csz_opt
                        .ok_or_else(|| "Verification failed: unknown-size Cluster child".to_string())?;
                    let cdata_start = cp + cid_len + csz_len;
                    let cend = cdata_start
                        .checked_add(csz)
                        .ok_or_else(|| "Cluster child end overflow".to_string())?;
                    if cend > eend {
                        return Err(format!(
                            "Verification failed: Cluster child 0x{:X} exceeds Cluster bounds ({} > {})",
                            cid, cend, eend
                        ));
                    }
                    if cid == 0xA3 {
                        let (tnum, tv_len) = stream_read_vint_u64(&mut reader)?;
                        if tv_len + 3 > csz {
                            return Err("Verification failed: truncated SimpleBlock header".to_string());
                        }
                        if tnum == 1 {
                            track1_blocks += 1;
                        } else if tnum == 2 {
                            track2_blocks += 1;
                        }
                    }
                    cp = cend;
                }
            }
            0x1C53BB6B => saw_cues = true,
            _ => {}
        }

        pos = eend;
    }

    let expected_tracks = if expected_audio_samples > 0 { 2 } else { 1 };
    if !saw_seek_head || !saw_info || !info_valid || !saw_tracks || tracks_count != expected_tracks {
        return Err(format!(
            "Verification failed: EBML metadata incomplete (seek={}, info={}, info_valid={}, tracks={}, count={}/{})",
            saw_seek_head, saw_info, info_valid, saw_tracks, tracks_count, expected_tracks
        ));
    }
    if cluster_count == 0 || track1_blocks != expected_video_samples || track2_blocks != expected_audio_samples {
        return Err(format!(
            "Verification failed: EBML block counts mismatch (clusters={}, t1={}/{}, t2={}/{})",
            cluster_count, track1_blocks, expected_video_samples, track2_blocks, expected_audio_samples
        ));
    }
    if expected_video_samples > 0 && !saw_cues {
        // Cues should be present whenever at least one keyframe exists; non-fatal if stream had 0 keyframes
    }

    Ok(file_len)
}

fn build_webm_or_mkv_tracks(
    video_track: &ParsedIsoTrack,
    audio_track: Option<&ParsedIsoTrack>,
) -> Result<Vec<u8>, String> {
    let mut tracks_body = Vec::new();

    // Track 1: Video
    let v_codec_id = match video_track.codec {
        TrackCodec::Vp9 => "V_VP9",
        TrackCodec::Vp8 => "V_VP8",
        TrackCodec::Av1 => "V_AV1",
        TrackCodec::Avc => "V_MPEG4/ISO/AVC",
        TrackCodec::Hevc => "V_MPEGH/ISO/HEVC",
        other => {
            return Err(format!(
                "Unsupported video codec {} for Matroska/WebM output",
                other.as_display_name()
            ))
        }
    };

    let mut v_priv = video_track.codec_private.clone();
    if v_priv.as_ref().map(|v| v.is_empty()).unwrap_or(true) && video_track.codec == TrackCodec::Avc {
        if let MediaConfig::AvcConfig(ref cfg) = video_track.media_conf {
            v_priv = build_avcc_from_config(cfg);
        }
    }

    let v_width = video_track.width.max(1);
    let v_height = video_track.height.max(1);
    let d_width = video_track.display_width.max(v_width);
    let d_height = video_track.display_height.max(v_height);

    let mut v_settings = Vec::new();
    v_settings.extend_from_slice(&make_ebml(&[0xB0], &encode_ebml_uint(v_width as u64)));
    v_settings.extend_from_slice(&make_ebml(&[0xBA], &encode_ebml_uint(v_height as u64)));
    v_settings.extend_from_slice(&make_ebml(&[0x54, 0xB0], &encode_ebml_uint(d_width as u64)));
    v_settings.extend_from_slice(&make_ebml(&[0x54, 0xBA], &encode_ebml_uint(d_height as u64)));
    v_settings.extend_from_slice(&make_ebml(&[0x54, 0xB2], &[0])); // pixels

    let default_dur_ns = if video_track.default_duration_ns > 0 {
        video_track.default_duration_ns
    } else if let Some(first) = video_track.samples.first() {
        (first.duration as u64)
            .saturating_mul(1_000_000_000)
            / (video_track.timescale.max(1) as u64)
    } else {
        0
    };

    let mut v_entry = Vec::new();
    v_entry.extend_from_slice(&make_ebml(&[0xD7], &[1])); // TrackNumber = 1
    v_entry.extend_from_slice(&make_ebml(&[0x73, 0xC5], &[1])); // TrackUID = 1
    v_entry.extend_from_slice(&make_ebml(&[0x83], &[1])); // TrackType = Video
    v_entry.extend_from_slice(&make_ebml(&[0x88], &[1])); // FlagDefault = 1
    v_entry.extend_from_slice(&make_ebml(&[0x9C], &[0])); // FlagLacing = 0
    v_entry.extend_from_slice(&make_ebml(&[0x86], v_codec_id.as_bytes()));
    if !video_track.language.is_empty() {
        v_entry.extend_from_slice(&make_ebml(&[0x22, 0xB5, 0x9C], video_track.language.as_bytes()));
    }
    if default_dur_ns > 0 {
        v_entry.extend_from_slice(&make_ebml(&[0x23, 0xE3, 0x83], &encode_ebml_uint(default_dur_ns)));
    }
    if let Some(ref cp) = v_priv {
        if !cp.is_empty() {
            v_entry.extend_from_slice(&make_ebml(&[0x63, 0xA2], cp));
        }
    }
    v_entry.extend_from_slice(&make_ebml(&[0xE0], &v_settings));
    tracks_body.extend_from_slice(&make_ebml(&[0xAE], &v_entry));

    // Track 2: Audio
    if let Some(audio) = audio_track {
        let a_codec_id = match audio.codec {
            TrackCodec::Opus => "A_OPUS",
            TrackCodec::Vorbis => "A_VORBIS",
            TrackCodec::Aac => "A_AAC",
            other => {
                return Err(format!(
                    "Unsupported audio codec {} for Matroska/WebM output",
                    other.as_display_name()
                ))
            }
        };

        let mut a_priv = audio.codec_private.clone();
        let channels = audio.channels.max(1);
        let sample_rate = if audio.sampling_frequency > 0.0 {
            audio.sampling_frequency
        } else if let MediaConfig::AacConfig(ref ac) = audio.media_conf {
            ac.freq_index.freq() as f64
        } else {
            48_000.0
        };

        if a_priv.as_ref().map(|v| v.is_empty()).unwrap_or(true) {
            if audio.codec == TrackCodec::Opus {
                a_priv = Some(synthesize_default_opus_head(channels, sample_rate.round() as u32));
            } else if audio.codec == TrackCodec::Aac {
                if let MediaConfig::AacConfig(ref ac) = audio.media_conf {
                    a_priv = Some(synthesize_aac_asc(ac));
                }
            }
        }

        let mut a_settings = Vec::new();
        a_settings.extend_from_slice(&make_ebml(&[0xB5], &sample_rate.to_be_bytes()));
        a_settings.extend_from_slice(&make_ebml(&[0x9F], &encode_ebml_uint(channels as u64)));
        if let Some(bd) = audio.bit_depth {
            if bd > 0 {
                a_settings.extend_from_slice(&make_ebml(&[0x62, 0x64], &encode_ebml_uint(bd as u64)));
            }
        }

        let mut a_entry = Vec::new();
        a_entry.extend_from_slice(&make_ebml(&[0xD7], &[2])); // TrackNumber = 2
        a_entry.extend_from_slice(&make_ebml(&[0x73, 0xC5], &[2])); // TrackUID = 2
        a_entry.extend_from_slice(&make_ebml(&[0x83], &[2])); // TrackType = Audio
        a_entry.extend_from_slice(&make_ebml(&[0x88], &[1])); // FlagDefault = 1
        a_entry.extend_from_slice(&make_ebml(&[0x9C], &[0])); // FlagLacing = 0
        a_entry.extend_from_slice(&make_ebml(&[0x86], a_codec_id.as_bytes()));
        if !audio.language.is_empty() {
            a_entry.extend_from_slice(&make_ebml(&[0x22, 0xB5, 0x9C], audio.language.as_bytes()));
        }
        if audio.codec == TrackCodec::Opus {
            let delay = if audio.codec_delay_ns > 0 {
                audio.codec_delay_ns
            } else {
                6_500_000
            };
            let preroll = if audio.seek_pre_roll_ns > 0 {
                audio.seek_pre_roll_ns
            } else {
                80_000_000
            };
            a_entry.extend_from_slice(&make_ebml(&[0x56, 0xAA], &encode_ebml_uint(delay)));
            a_entry.extend_from_slice(&make_ebml(&[0x56, 0xBB], &encode_ebml_uint(preroll)));
        } else {
            if audio.codec_delay_ns > 0 {
                a_entry.extend_from_slice(&make_ebml(&[0x56, 0xAA], &encode_ebml_uint(audio.codec_delay_ns)));
            }
            if audio.seek_pre_roll_ns > 0 {
                a_entry.extend_from_slice(&make_ebml(&[0x56, 0xBB], &encode_ebml_uint(audio.seek_pre_roll_ns)));
            }
        }
        if let Some(ref cp) = a_priv {
            if !cp.is_empty() {
                a_entry.extend_from_slice(&make_ebml(&[0x63, 0xA2], cp));
            }
        }
        a_entry.extend_from_slice(&make_ebml(&[0xE1], &a_settings));
        tracks_body.extend_from_slice(&make_ebml(&[0xAE], &a_entry));
    }

    Ok(make_ebml(&[0x16, 0x54, 0xAE, 0x6B], &tracks_body))
}

fn flush_ebml_cluster<W: Write + Seek>(
    writer: &mut W,
    cluster_ts_ms: u64,
    cluster_payload: &mut Vec<u8>,
    cues: &mut [(u64, u64, u64)],
    cue_start_idx: usize,
    seg_data_start: u64,
) -> Result<(), String> {
    if cluster_payload.is_empty() {
        return Ok(());
    }
    let cluster_file_pos = writer
        .stream_position()
        .map_err(|e| format!("Stream position error before Cluster: {}", e))?;
    let cluster_rel_pos = cluster_file_pos.saturating_sub(seg_data_start);

    let timecode_elem = make_ebml(&[0xE7], &encode_ebml_uint(cluster_ts_ms));
    let body_len = (timecode_elem.len() + cluster_payload.len()) as u64;
    let size_vint = encode_ebml_vint_size(body_len);

    // Update CueClusterPosition and shift CueRelativePosition by timecode_elem.len()
    let tc_shift = timecode_elem.len() as u64;
    for cue in &mut cues[cue_start_idx..] {
        cue.1 = cluster_rel_pos;
        cue.2 += tc_shift;
    }

    writer
        .write_all(&[0x1F, 0x43, 0xB6, 0x75])
        .map_err(|e| format!("Failed writing Cluster ID: {}", e))?;
    writer
        .write_all(&size_vint)
        .map_err(|e| format!("Failed writing Cluster size: {}", e))?;
    writer
        .write_all(&timecode_elem)
        .map_err(|e| format!("Failed writing Cluster Timestamp: {}", e))?;
    writer
        .write_all(cluster_payload)
        .map_err(|e| format!("Failed writing Cluster blocks: {}", e))?;
    cluster_payload.clear();
    Ok(())
}

/// Pure-Rust streaming Matroska / WebM muxer writing EBML Header, SeekHead, Info, Tracks,
/// interleaved Clusters (~4 MiB flush threshold + safe i16 relative timestamp splits), and Cues.
pub fn write_muxed_matroska_or_webm(
    video_track: &ParsedIsoTrack,
    audio_track: Option<&ParsedIsoTrack>,
    container: OutputContainer,
    output_path: &Path,
) -> Result<u64, String> {
    if container == OutputContainer::Mp4 {
        return write_muxed_mp4(video_track, audio_track, output_path);
    }
    if video_track.samples.is_empty() {
        return Err("Cannot mux 0 video samples".to_string());
    }
    if let Some(parent) = output_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let tmp_path = make_part_output_path(output_path);
    let result = (|| -> Result<u64, String> {
        let file = File::create(&tmp_path)
            .map_err(|e| format!("Failed creating staging container {}: {}", tmp_path.display(), e))?;
        let mut writer = BufWriter::with_capacity(1024 * 1024, file);

        // 1. EBML Header
        let doc_type = container.ebml_doc_type();
        let mut ebml_hdr_body = Vec::new();
        ebml_hdr_body.extend_from_slice(&make_ebml(&[0x42, 0x86], &[1])); // EBMLVersion
        ebml_hdr_body.extend_from_slice(&make_ebml(&[0x42, 0xF7], &[1])); // EBMLReadVersion
        ebml_hdr_body.extend_from_slice(&make_ebml(&[0x42, 0xF2], &[4])); // EBMLMaxIDLength
        ebml_hdr_body.extend_from_slice(&make_ebml(&[0x42, 0xF3], &[8])); // EBMLMaxSizeLength
        ebml_hdr_body.extend_from_slice(&make_ebml(&[0x42, 0x82], doc_type.as_bytes()));
        ebml_hdr_body.extend_from_slice(&make_ebml(&[0x42, 0x87], &[4])); // DocTypeVersion
        ebml_hdr_body.extend_from_slice(&make_ebml(&[0x42, 0x85], &[2])); // DocTypeReadVersion
        let ebml_hdr = make_ebml(&[0x1A, 0x45, 0xDF, 0xA3], &ebml_hdr_body);
        writer
            .write_all(&ebml_hdr)
            .map_err(|e| format!("Failed writing EBML header: {}", e))?;

        // 2. Segment header (8-byte unknown-size VINT placeholder)
        writer
            .write_all(&[0x18, 0x53, 0x80, 0x67])
            .map_err(|e| format!("Failed writing Segment ID: {}", e))?;
        let seg_size_pos = writer
            .stream_position()
            .map_err(|e| format!("Stream position error: {}", e))?;
        writer
            .write_all(&0x01FF_FFFF_FFFF_FFFFu64.to_be_bytes())
            .map_err(|e| format!("Failed writing Segment size placeholder: {}", e))?;
        let seg_data_start = seg_size_pos + 8;

        // 3. Fixed-size SeekHead placeholder
        let seek_head_pos = seg_data_start;
        let seek_placeholder = build_seek_head_bytes(0, 0, 0);
        writer
            .write_all(&seek_placeholder)
            .map_err(|e| format!("Failed writing SeekHead placeholder: {}", e))?;

        // 4. Info
        let info_pos = writer
            .stream_position()
            .map_err(|e| format!("Stream position error before Info: {}", e))?;
        let v_scale = video_track.timescale.max(1) as f64;
        let a_scale = audio_track
            .map(|a| a.timescale.max(1) as f64)
            .unwrap_or(1.0);
        let v_dur_ms = video_track
            .samples
            .iter()
            .map(|s| ((s.start_time as f64 + s.duration as f64) * 1000.0) / v_scale)
            .fold(0.0f64, f64::max);
        let a_dur_ms = audio_track
            .map(|a| {
                a.samples
                    .iter()
                    .map(|s| ((s.start_time as f64 + s.duration as f64) * 1000.0) / a_scale)
                    .fold(0.0f64, f64::max)
            })
            .unwrap_or(0.0);
        let total_dur_ms = v_dur_ms.max(a_dur_ms).max(1.0);

        let mut info_body = Vec::new();
        info_body.extend_from_slice(&make_ebml(
            &[0x2A, 0xD7, 0xB1],
            &encode_ebml_uint(1_000_000),
        ));
        info_body.extend_from_slice(&make_ebml(&[0x44, 0x89], &total_dur_ms.to_be_bytes()));
        info_body.extend_from_slice(&make_ebml(&[0x4D, 0x80], b"OctoHeft"));
        info_body.extend_from_slice(&make_ebml(&[0x57, 0x41], b"OctoHeft"));
        let info_elem = make_ebml(&[0x15, 0x49, 0xA9, 0x66], &info_body);
        writer
            .write_all(&info_elem)
            .map_err(|e| format!("Failed writing Info element: {}", e))?;

        // 5. Tracks
        let tracks_pos = writer
            .stream_position()
            .map_err(|e| format!("Stream position error before Tracks: {}", e))?;
        let tracks_elem = build_webm_or_mkv_tracks(video_track, audio_track)?;
        writer
            .write_all(&tracks_elem)
            .map_err(|e| format!("Failed writing Tracks element: {}", e))?;

        // 6. Stream Interleaved Clusters directly to disk (~4 MiB rolling buffer)
        let v_scale_u = video_track.timescale.max(1) as u128;
        let a_scale_u = audio_track
            .map(|a| a.timescale.max(1) as u128)
            .unwrap_or(1);
        let a_samples = audio_track.map(|a| a.samples.as_slice()).unwrap_or(&[]);

        let mut vi = 0usize;
        let mut ai = 0usize;
        let mut cluster_ts_ms: Option<u64> = None;
        let mut cluster_payload: Vec<u8> = Vec::with_capacity(256 * 1024);
        let mut cues: Vec<(u64, u64, u64)> = Vec::new(); // (pts_ms, cluster_rel_pos, block_rel_pos)
        let mut cluster_cue_start = 0usize;

        while vi < video_track.samples.len() || ai < a_samples.len() {
            let v_dts_ms = video_track
                .samples
                .get(vi)
                .map(|s| ((s.start_time as u128 * 1000) / v_scale_u) as u64)
                .unwrap_or(u64::MAX);
            let a_dts_ms = a_samples
                .get(ai)
                .map(|s| ((s.start_time as u128 * 1000) / a_scale_u) as u64)
                .unwrap_or(u64::MAX);

            let (track_vint, is_video, is_key, pts_ms, sample_bytes): (u8, bool, bool, u64, &Bytes) =
                if v_dts_ms <= a_dts_ms {
                    let s = &video_track.samples[vi];
                    let pts_ticks = ((s.start_time as i128) + (s.rendering_offset as i128)).max(0) as u128;
                    let pts_ms = ((pts_ticks * 1000) / v_scale_u) as u64;
                    vi += 1;
                    (0x81, true, s.is_sync, pts_ms, &s.bytes)
                } else {
                    let s = &a_samples[ai];
                    let pts_ms = ((s.start_time as u128 * 1000) / a_scale_u) as u64;
                    ai += 1;
                    (0x82, false, true, pts_ms, &s.bytes)
                };

            let need_new_cluster = match cluster_ts_ms {
                None => true,
                Some(cur_tc) => {
                    let diff = (pts_ms as i64) - (cur_tc as i64);
                    diff > 30_000
                        || diff < -10_000
                        || (is_video
                            && is_key
                            && !cluster_payload.is_empty()
                            && (diff >= 2_000 || cluster_payload.len() >= 4 * 1024 * 1024))
                }
            };

            if need_new_cluster {
                if let Some(cur_tc) = cluster_ts_ms {
                    flush_ebml_cluster(
                        &mut writer,
                        cur_tc,
                        &mut cluster_payload,
                        &mut cues,
                        cluster_cue_start,
                        seg_data_start,
                    )?;
                }
                cluster_ts_ms = Some(pts_ms);
                cluster_cue_start = cues.len();
            }

            let cur_tc = cluster_ts_ms.unwrap_or(pts_ms);
            let rel_tc = ((pts_ms as i64) - (cur_tc as i64)).clamp(i16::MIN as i64, i16::MAX as i64) as i16;

            if is_video && is_key {
                cues.push((pts_ms, 0, cluster_payload.len() as u64));
            }

            let sb_body_len = 4u64 + (sample_bytes.len() as u64);
            let sb_size_vint = encode_ebml_vint_size(sb_body_len);
            cluster_payload.push(0xA3);
            cluster_payload.extend_from_slice(&sb_size_vint);
            cluster_payload.push(track_vint);
            cluster_payload.extend_from_slice(&rel_tc.to_be_bytes());
            cluster_payload.push(if is_key { 0x80 } else { 0x00 });
            cluster_payload.extend_from_slice(sample_bytes);
        }

        if let Some(cur_tc) = cluster_ts_ms {
            flush_ebml_cluster(
                &mut writer,
                cur_tc,
                &mut cluster_payload,
                &mut cues,
                cluster_cue_start,
                seg_data_start,
            )?;
        }

        // 7. Write Cues index
        let cues_pos = writer
            .stream_position()
            .map_err(|e| format!("Stream position error before Cues: {}", e))?;
        let mut cues_body = Vec::new();
        for (cue_ts, cluster_rel, block_rel) in &cues {
            let mut tp_body = Vec::new();
            tp_body.extend_from_slice(&make_ebml(&[0xF7], &[1])); // CueTrack = 1
            tp_body.extend_from_slice(&make_ebml(&[0xF1], &encode_ebml_uint(*cluster_rel)));
            tp_body.extend_from_slice(&make_ebml(&[0xF0], &encode_ebml_uint(*block_rel)));
            let mut cp_body = Vec::new();
            cp_body.extend_from_slice(&make_ebml(&[0xB3], &encode_ebml_uint(*cue_ts)));
            cp_body.extend_from_slice(&make_ebml(&[0xB7], &tp_body));
            cues_body.extend_from_slice(&make_ebml(&[0xBB], &cp_body));
        }
        if !cues_body.is_empty() {
            let cues_elem = make_ebml(&[0x1C, 0x53, 0xBB, 0x6B], &cues_body);
            writer
                .write_all(&cues_elem)
                .map_err(|e| format!("Failed writing Cues element: {}", e))?;
        }

        writer
            .flush()
            .map_err(|e| format!("Failed flushing EBML stream: {}", e))?;
        let seg_end_pos = writer
            .stream_position()
            .map_err(|e| format!("Failed reading Segment end position: {}", e))?;
        let seg_payload_len = seg_end_pos.saturating_sub(seg_data_start);

        // 8. Patch Segment size and SeekHead offsets in place
        writer
            .seek(SeekFrom::Start(seg_size_pos))
            .map_err(|e| format!("Failed seeking to Segment size: {}", e))?;
        writer
            .write_all(&(0x0100_0000_0000_0000u64 | seg_payload_len).to_be_bytes())
            .map_err(|e| format!("Failed patching Segment size: {}", e))?;

        let real_seek_head = build_seek_head_bytes(
            info_pos.saturating_sub(seg_data_start),
            tracks_pos.saturating_sub(seg_data_start),
            cues_pos.saturating_sub(seg_data_start),
        );
        writer
            .seek(SeekFrom::Start(seek_head_pos))
            .map_err(|e| format!("Failed seeking to SeekHead: {}", e))?;
        writer
            .write_all(&real_seek_head)
            .map_err(|e| format!("Failed patching SeekHead: {}", e))?;

        writer
            .flush()
            .map_err(|e| format!("Failed final flush on EBML container: {}", e))?;
        drop(writer);

        let a_expected = audio_track.map(|a| a.samples.len()).unwrap_or(0);
        verify_ebml_container_file(&tmp_path, doc_type, video_track.samples.len(), a_expected)
    })();

    match result {
        Ok(verified_len) => {
            rename_part_to_final(&tmp_path, output_path)?;
            Ok(verified_len)
        }
        Err(err) => {
            let _ = std::fs::remove_file(&tmp_path);
            Err(err)
        }
    }
}

pub fn write_muxed_mp4(
    video_track: &ParsedIsoTrack,
    audio_track: Option<&ParsedIsoTrack>,
    output_path: &Path,
) -> Result<u64, String> {
    if video_track.codec != TrackCodec::Avc {
        return Err(format!(
            "Refusing to force non-AVC codec '{}' into legacy MP4 box writer; use WebM/Matroska container routing",
            video_track.codec.as_display_name()
        ));
    }
    if let Some(a) = audio_track {
        if a.codec != TrackCodec::Aac {
            return Err(format!(
                "Refusing to write non-AAC audio '{}' into MP4 writer; use WebM/Matroska container routing",
                a.codec.as_display_name()
            ));
        }
    }

    if let Some(parent) = output_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let tmp_path = make_part_output_path(output_path);
    let result = (|| -> Result<u64, String> {
        {
            let file = File::create(&tmp_path)
                .map_err(|e| format!("Failed to create output file: {}", e))?;
            let mut buf_writer = BufWriter::new(file);

            let mp4_config = Mp4Config {
                major_brand: str::parse("isom").unwrap(),
                minor_version: 512,
                compatible_brands: vec![
                    str::parse("isom").unwrap(),
                    str::parse("iso2").unwrap(),
                    str::parse("avc1").unwrap(),
                    str::parse("mp41").unwrap(),
                ],
                timescale: 1000,
            };

            let mut writer = Mp4Writer::write_start(&mut buf_writer, &mp4_config)
                .map_err(|e| format!("Mp4Writer init error: {}", e))?;

            let video_cfg = TrackConfig {
                track_type: video_track.track_type,
                timescale: video_track.timescale.max(1),
                language: video_track.language.clone(),
                media_conf: video_track.media_conf.clone(),
            };
            writer
                .add_track(&video_cfg)
                .map_err(|e| format!("Failed to add video track: {}", e))?;

            if let Some(audio) = audio_track {
                let audio_cfg = TrackConfig {
                    track_type: audio.track_type,
                    timescale: audio.timescale.max(1),
                    language: audio.language.clone(),
                    media_conf: audio.media_conf.clone(),
                };
                writer
                    .add_track(&audio_cfg)
                    .map_err(|e| format!("Failed to add audio track: {}", e))?;
            }

            // Interleave A/V samples chronologically in milliseconds
            let v_scale = video_track.timescale.max(1) as u128;
            let a_scale = audio_track.map(|a| a.timescale.max(1) as u128).unwrap_or(1);

            let mut vi = 0usize;
            let mut ai = 0usize;
            let a_samples = audio_track.map(|a| a.samples.as_slice()).unwrap_or(&[]);

            while vi < video_track.samples.len() || ai < a_samples.len() {
                let v_ms = video_track
                    .samples
                    .get(vi)
                    .map(|s| ((s.start_time as u128).saturating_mul(1000) / v_scale) as u64)
                    .unwrap_or(u64::MAX);
                let a_ms = a_samples
                    .get(ai)
                    .map(|s| ((s.start_time as u128).saturating_mul(1000) / a_scale) as u64)
                    .unwrap_or(u64::MAX);

                if v_ms <= a_ms {
                    let s = &video_track.samples[vi];
                    let mp4_sample = Mp4Sample {
                        start_time: s.start_time,
                        duration: s.duration,
                        rendering_offset: s.rendering_offset,
                        is_sync: s.is_sync,
                        bytes: s.bytes.clone(),
                    };
                    writer
                        .write_sample(1, &mp4_sample)
                        .map_err(|e| format!("Failed writing video sample: {}", e))?;
                    vi += 1;
                } else {
                    let s = &a_samples[ai];
                    let mp4_sample = Mp4Sample {
                        start_time: s.start_time,
                        duration: s.duration,
                        rendering_offset: s.rendering_offset,
                        is_sync: s.is_sync,
                        bytes: s.bytes.clone(),
                    };
                    writer
                        .write_sample(2, &mp4_sample)
                        .map_err(|e| format!("Failed writing audio sample: {}", e))?;
                    ai += 1;
                }
            }

            writer
                .write_end()
                .map_err(|e| format!("Failed to finalize MP4: {}", e))?;
            buf_writer
                .flush()
                .map_err(|e| format!("Failed to flush output: {}", e))?;
        }

        let a_cnt = audio_track.map(|a| a.samples.len()).unwrap_or(0);
        verify_mp4_container_file(&tmp_path, video_track.samples.len(), a_cnt)
    })();

    match result {
        Ok(verified_len) => {
            rename_part_to_final(&tmp_path, output_path)?;
            Ok(verified_len)
        }
        Err(err) => {
            let _ = std::fs::remove_file(&tmp_path);
            Err(err)
        }
    }
}

pub fn write_muxed_container(
    video_track: &ParsedIsoTrack,
    audio_track: Option<&ParsedIsoTrack>,
    destination_path: &Path,
) -> Result<(PathBuf, u64), String> {
    let container = select_output_container(video_track, audio_track)?;
    let final_path = resolve_output_path_for_container(destination_path, container);
    let len = match container {
        OutputContainer::Mp4 => write_muxed_mp4(video_track, audio_track, &final_path)?,
        OutputContainer::WebM | OutputContainer::Matroska => {
            write_muxed_matroska_or_webm(video_track, audio_track, container, &final_path)?
        }
    };
    Ok((final_path, len))
}

fn validate_media_payload(bytes: &[u8], label: &str) -> Result<(), String> {
    if bytes.len() < 16 {
        return Err(format!(
            "{} stream too short to be valid media ({} bytes)",
            label,
            bytes.len()
        ));
    }
    let is_ebml = bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]);
    let has_box = bytes.windows(4).take(64).any(|w| w == b"ftyp" || w == b"moov");
    if !is_ebml && !has_box {
        return Err(format!(
            "{} stream did not start with ISOBMFF (ftyp/moov) or WebM/EBML header",
            label
        ));
    }
    Ok(())
}

fn parse_clen_from_url(url_str: &str) -> Option<u64> {
    Url::parse(url_str).ok().and_then(|u| {
        u.query_pairs()
            .find(|(k, _)| k == "clen")
            .and_then(|(_, v)| v.parse::<u64>().ok())
    })
}

const MAX_SUBRANGE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_CONSECUTIVE_NO_PROGRESS: usize = 12;
const CHUNK_READ_TIMEOUT: Duration = Duration::from_secs(30);

fn parse_content_range_start(resp: &reqwest::Response) -> Option<u64> {
    resp.headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|cr| cr.trim().strip_prefix("bytes "))
        .and_then(|rest| rest.split('-').next())
        .and_then(|s| s.trim().parse::<u64>().ok())
}

enum ProbeResult {
    Partial {
        total_len: u64,
        first_byte: Option<u8>,
    },
    FullStream(reqwest::Response),
}

async fn probe_media_stream(
    client: &reqwest::Client,
    url: &str,
    headers: Option<&HashMap<String, String>>,
) -> Result<ProbeResult, String> {
    let mut last_err = String::new();
    for attempt in 0..4usize {
        let send_res = send_request_with_headers(
            client,
            reqwest::Method::GET,
            url,
            headers,
            Some("bytes=0-0"),
        )
        .await;

        let resp = match send_res {
            Ok(r) => r,
            Err(e) => {
                last_err = format!("Failed connecting to media stream {}: {}", url, e);
                tokio::time::sleep(Duration::from_millis(250 * ((attempt as u64) + 1))).await;
                continue;
            }
        };

        let status = resp.status();
        if status == reqwest::StatusCode::PARTIAL_CONTENT {
            let total_from_cr = resp
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(|cr| cr.rsplit('/').next())
                .and_then(|s| s.parse::<u64>().ok());

            let first_bytes = resp
                .bytes()
                .await
                .map_err(|e| format!("Failed reading probe byte: {}", e))?;

            let total_len = total_from_cr
                .or_else(|| parse_clen_from_url(url))
                .unwrap_or(0);

            return Ok(ProbeResult::Partial {
                total_len,
                first_byte: first_bytes.first().copied(),
            });
        }

        if status.is_success() {
            return Ok(ProbeResult::FullStream(resp));
        }

        if matches!(
            status,
            reqwest::StatusCode::UNAUTHORIZED
                | reqwest::StatusCode::FORBIDDEN
                | reqwest::StatusCode::NOT_FOUND
                | reqwest::StatusCode::GONE
        ) {
            return Err(format!("Media stream returned HTTP {}", status));
        }

        if matches!(
            status,
            reqwest::StatusCode::TOO_MANY_REQUESTS
                | reqwest::StatusCode::BAD_GATEWAY
                | reqwest::StatusCode::SERVICE_UNAVAILABLE
                | reqwest::StatusCode::GATEWAY_TIMEOUT
        ) || status.is_server_error()
        {
            last_err = format!("Media stream probe transient HTTP {}", status);
            tokio::time::sleep(Duration::from_millis(300 * ((attempt as u64) + 1))).await;
            continue;
        }

        // Some CDNs reject Range: bytes=0-0 with 416/400 -> retry plain GET
        let fallback_resp =
            send_request_with_headers(client, reqwest::Method::GET, url, headers, None)
                .await
                .map_err(|e| format!("Failed fetching media stream {}: {}", url, e))?;
        if !fallback_resp.status().is_success() {
            return Err(format!(
                "Media stream returned HTTP {}",
                fallback_resp.status()
            ));
        }
        return Ok(ProbeResult::FullStream(fallback_resp));
    }

    Err(last_err)
}

async fn fetch_range_with_retry(
    client: &reqwest::Client,
    url: &str,
    headers: Option<&HashMap<String, String>>,
    req_start: u64,
    req_end: u64,
    seg_id: usize,
    downloaded_counter: Arc<AtomicU64>,
    segments_state: Arc<Mutex<Vec<SegmentProgress>>>,
    notify_tx: mpsc::UnboundedSender<()>,
) -> Result<Vec<u8>, String> {
    let total_expected_u64 = req_end.saturating_sub(req_start) + 1;
    let total_expected = usize::try_from(total_expected_u64)
        .map_err(|_| format!("Segment {} range size overflow ({} bytes)", seg_id, total_expected_u64))?;
    let mut buf = Vec::with_capacity(total_expected);
    let mut last_err = String::new();
    let mut consecutive_no_progress = 0usize;

    while (buf.len() as u64) < total_expected_u64 {
        let cur_pos = req_start + (buf.len() as u64);
        let sub_end = (cur_pos + MAX_SUBRANGE_BYTES - 1).min(req_end);
        let range_header = format!("bytes={}-{}", cur_pos, sub_end);

        let send_res = send_request_with_headers(
            client,
            reqwest::Method::GET,
            url,
            headers,
            Some(&range_header),
        )
        .await;

        let resp = match send_res {
            Ok(r) => r,
            Err(e) => {
                consecutive_no_progress += 1;
                last_err = format!("Range {}-{} request failed: {}", cur_pos, sub_end, e);
                warn!(seg_id, cur_pos, sub_end, consecutive_no_progress, error = %e, "Range request error");
                if consecutive_no_progress >= MAX_CONSECUTIVE_NO_PROGRESS {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(
                    250 * (consecutive_no_progress.min(8) as u64),
                ))
                .await;
                continue;
            }
        };

        let status = resp.status();
        if !status.is_success() {
            last_err = format!("Range {}-{} returned HTTP {}", cur_pos, sub_end, status);
            if matches!(
                status,
                reqwest::StatusCode::UNAUTHORIZED
                    | reqwest::StatusCode::FORBIDDEN
                    | reqwest::StatusCode::NOT_FOUND
                    | reqwest::StatusCode::GONE
            ) {
                break;
            }
            consecutive_no_progress += 1;
            warn!(seg_id, cur_pos, sub_end, status = %status, consecutive_no_progress, "Transient HTTP error");
            if consecutive_no_progress >= MAX_CONSECUTIVE_NO_PROGRESS {
                break;
            }
            tokio::time::sleep(Duration::from_millis(
                300 * (consecutive_no_progress.min(8) as u64),
            ))
            .await;
            continue;
        }

        let mut skip_remaining: u64 = if status == reqwest::StatusCode::PARTIAL_CONTENT {
            if let Some(cr_start) = parse_content_range_start(&resp) {
                if cr_start > cur_pos {
                    consecutive_no_progress += 1;
                    last_err = format!(
                        "Server skipped forward in Content-Range: got start {}, expected {}",
                        cr_start, cur_pos
                    );
                    if consecutive_no_progress >= MAX_CONSECUTIVE_NO_PROGRESS {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    continue;
                }
                cur_pos - cr_start
            } else {
                0
            }
        } else {
            // Server ignored Range and returned 200 OK from byte 0
            cur_pos
        };

        let mut bytes_this_attempt = 0usize;
        let sub_target_len = buf.len() + ((sub_end - cur_pos + 1) as usize);
        let mut stream = resp.bytes_stream();
        let mut stream_err: Option<String> = None;
        let mut t0 = Instant::now();
        let mut bytes_in_window: u64 = 0;

        while buf.len() < sub_target_len && buf.len() < total_expected {
            let next_chunk = match tokio::time::timeout(CHUNK_READ_TIMEOUT, stream.next()).await {
                Ok(Some(res)) => res,
                Ok(None) => break,
                Err(_) => {
                    stream_err = Some(format!(
                        "Stream read timed out after {}s at offset {}",
                        CHUNK_READ_TIMEOUT.as_secs(),
                        req_start + (buf.len() as u64)
                    ));
                    break;
                }
            };

            match next_chunk {
                Ok(chunk) => {
                    let mut slice = &chunk[..];
                    if skip_remaining > 0 {
                        let skip_now = (slice.len() as u64).min(skip_remaining) as usize;
                        slice = &slice[skip_now..];
                        skip_remaining -= skip_now as u64;
                        if slice.is_empty() {
                            continue;
                        }
                    }

                    let room = total_expected
                        .saturating_sub(buf.len())
                        .min(sub_target_len.saturating_sub(buf.len()));
                    let take = slice.len().min(room);
                    if take == 0 {
                        break;
                    }
                    buf.extend_from_slice(&slice[..take]);
                    bytes_this_attempt += take;
                    downloaded_counter.fetch_add(take as u64, Ordering::Relaxed);
                    bytes_in_window += take as u64;

                    let dt = t0.elapsed().as_secs_f64();
                    let seg_speed = if dt >= 0.2 {
                        let s = (bytes_in_window as f64 / dt) as u64;
                        bytes_in_window = 0;
                        t0 = Instant::now();
                        Some(s)
                    } else {
                        None
                    };

                    {
                        let mut guard = segments_state.lock();
                        if let Some(seg) = guard.iter_mut().find(|s| s.id == seg_id) {
                            seg.downloaded += take as u64;
                            seg.current_byte = seg.start_byte + seg.downloaded;
                            seg.status = SegmentStatus::Downloading;
                            if let Some(spd) = seg_speed {
                                seg.speed = spd;
                            }
                        }
                    }
                    let _ = notify_tx.send(());
                }
                Err(e) => {
                    stream_err = Some(format!(
                        "Range {}-{} stream error at offset {}: {}",
                        cur_pos,
                        sub_end,
                        req_start + (buf.len() as u64),
                        e
                    ));
                    break;
                }
            }
        }

        if let Some(err_msg) = stream_err {
            last_err = err_msg;
        }

        if buf.len() == total_expected {
            break;
        }

        if bytes_this_attempt > 0 {
            consecutive_no_progress = 0;
            if buf.len() < sub_target_len {
                last_err = format!(
                    "Range {}-{} ended early after {} bytes (total {}/{})",
                    cur_pos,
                    sub_end,
                    bytes_this_attempt,
                    buf.len(),
                    total_expected
                );
                warn!(seg_id, cur_pos, sub_end, received = bytes_this_attempt, progress = buf.len(), total = total_expected, "Resuming mid-range after drop");
                tokio::time::sleep(Duration::from_millis(120)).await;
            }
        } else {
            consecutive_no_progress += 1;
            if last_err.is_empty() {
                last_err = format!(
                    "Range {}-{} yielded 0 bytes ({}/{})",
                    cur_pos,
                    sub_end,
                    buf.len(),
                    total_expected
                );
            }
            if consecutive_no_progress >= MAX_CONSECUTIVE_NO_PROGRESS {
                break;
            }
            tokio::time::sleep(Duration::from_millis(
                250 * (consecutive_no_progress.min(8) as u64),
            ))
            .await;
        }
    }

    if buf.len() == total_expected {
        let mut guard = segments_state.lock();
        if let Some(seg) = guard.iter_mut().find(|s| s.id == seg_id) {
            seg.downloaded = seg.end_byte.saturating_sub(seg.start_byte) + 1;
            seg.current_byte = seg.end_byte + 1;
            seg.speed = 0;
            seg.status = SegmentStatus::Completed;
        }
        let _ = notify_tx.send(());
        return Ok(buf);
    }

    {
        let mut guard = segments_state.lock();
        if let Some(seg) = guard.iter_mut().find(|s| s.id == seg_id) {
            seg.status = SegmentStatus::Failed;
            seg.speed = 0;
        }
    }
    let _ = notify_tx.send(());
    if last_err.is_empty() {
        last_err = format!(
            "Segment {} incomplete ({}/{} bytes)",
            seg_id,
            buf.len(),
            total_expected
        );
    }
    Err(last_err)
}

async fn consume_full_response_with_resume(
    client: &reqwest::Client,
    url: &str,
    headers: Option<&HashMap<String, String>>,
    expected_hint: u64,
    resp: reqwest::Response,
    seg_id: usize,
    downloaded_counter: Arc<AtomicU64>,
    total_counter: Arc<AtomicU64>,
    segments_state: Arc<Mutex<Vec<SegmentProgress>>>,
    notify_tx: mpsc::UnboundedSender<()>,
) -> Result<Vec<u8>, String> {
    let cl = resp
        .content_length()
        .filter(|&c| c > 0)
        .or_else(|| (expected_hint > 0).then_some(expected_hint))
        .unwrap_or(0);
    if cl > 0 {
        let mut guard = segments_state.lock();
        if let Some(seg) = guard.iter_mut().find(|s| s.id == seg_id) {
            seg.end_byte = seg.start_byte + cl - 1;
        }
    }

    let cap = cl.max(256 * 1024).min(64 * 1024 * 1024) as usize;
    let mut buf = Vec::with_capacity(cap);
    let mut last_err: Option<String> = None;

    {
        let mut stream = resp.bytes_stream();
        let mut t0 = Instant::now();
        let mut bytes_in_window: u64 = 0;

        loop {
            match tokio::time::timeout(CHUNK_READ_TIMEOUT, stream.next()).await {
                Ok(Some(Ok(chunk))) => {
                    buf.extend_from_slice(&chunk);
                    downloaded_counter.fetch_add(chunk.len() as u64, Ordering::Relaxed);
                    if cl == 0 {
                        total_counter.fetch_add(chunk.len() as u64, Ordering::Relaxed);
                    }
                    bytes_in_window += chunk.len() as u64;

                    let dt = t0.elapsed().as_secs_f64();
                    let seg_speed = if dt >= 0.2 {
                        let s = (bytes_in_window as f64 / dt) as u64;
                        bytes_in_window = 0;
                        t0 = Instant::now();
                        Some(s)
                    } else {
                        None
                    };

                    {
                        let mut guard = segments_state.lock();
                        if let Some(seg) = guard.iter_mut().find(|s| s.id == seg_id) {
                            seg.downloaded = buf.len() as u64;
                            seg.current_byte = seg.start_byte + seg.downloaded;
                            seg.status = SegmentStatus::Downloading;
                            if let Some(spd) = seg_speed {
                                seg.speed = spd;
                            }
                        }
                    }
                    let _ = notify_tx.send(());
                }
                Ok(Some(Err(e))) => {
                    last_err = Some(format!("Full stream read error at byte {}: {}", buf.len(), e));
                    break;
                }
                Ok(None) => break,
                Err(_) => {
                    last_err = Some(format!("Full stream timed out at byte {}", buf.len()));
                    break;
                }
            }
        }
    }

    // If expected Content-Length / clen is known and stream fell short (even on clean TCP EOF), recover via HTTP Range
    if cl > 0 && (buf.len() as u64) < cl {
        warn!(
            seg_id,
            received = buf.len(),
            expected = cl,
            reason = ?last_err,
            "Full stream terminated before Content-Length; attempting HTTP Range continuation resume"
        );
        let remaining_start = buf.len() as u64;
        let remaining_end = cl - 1;
        let tail = fetch_range_with_retry(
            client,
            url,
            headers,
            remaining_start,
            remaining_end,
            seg_id,
            Arc::clone(&downloaded_counter),
            Arc::clone(&segments_state),
            notify_tx.clone(),
        )
        .await
        .map_err(|resume_err| {
            format!(
                "Stream truncated ({}/{} bytes) and resume failed: {}",
                buf.len(),
                cl,
                resume_err
            )
        })?;
        buf.extend_from_slice(&tail);
    } else if let Some(err_msg) = last_err {
        let mut guard = segments_state.lock();
        if let Some(seg) = guard.iter_mut().find(|s| s.id == seg_id) {
            seg.status = SegmentStatus::Failed;
            seg.speed = 0;
        }
        let _ = notify_tx.send(());
        return Err(err_msg);
    }

    if cl > 0 && (buf.len() as u64) < cl {
        let mut guard = segments_state.lock();
        if let Some(seg) = guard.iter_mut().find(|s| s.id == seg_id) {
            seg.status = SegmentStatus::Failed;
            seg.speed = 0;
        }
        let _ = notify_tx.send(());
        return Err(format!(
            "Full stream incomplete after resume: got {} of {} bytes",
            buf.len(),
            cl
        ));
    }

    {
        let mut guard = segments_state.lock();
        if let Some(seg) = guard.iter_mut().find(|s| s.id == seg_id) {
            seg.end_byte = seg.start_byte + (buf.len() as u64).saturating_sub(1);
            seg.current_byte = seg.start_byte + buf.len() as u64;
            seg.downloaded = buf.len() as u64;
            seg.speed = 0;
            seg.status = SegmentStatus::Completed;
        }
    }
    let _ = notify_tx.send(());
    Ok(buf)
}

pub struct MediaMuxer;

impl MediaMuxer {
    /// Download split direct video + optional audio HTTP streams concurrently (e.g. YouTube MSE)
    /// via parallel bounded HTTP Range connections with byte-exact EOF resume, and mux in pure Rust
    /// into the codec-appropriate `.webm`, `.mkv`, or `.mp4` container.
    pub async fn download_and_remux_split_streams<F>(
        client: &reqwest::Client,
        video_url: &str,
        audio_url: Option<&str>,
        headers: Option<&HashMap<String, String>>,
        destination_path: &Path,
        mut progress_cb: F,
    ) -> Result<(PathBuf, u64), String>
    where
        F: FnMut(MuxProgress) + Send + 'static,
    {
        info!(
            destination = %destination_path.display(),
            has_audio = audio_url.is_some(),
            "Starting split A/V stream probe and mux pipeline"
        );

        progress_cb(MuxProgress {
            phase: "parsing".to_string(),
            segments_done: 0,
            segments_total: if audio_url.is_some() { 2 } else { 1 },
            percent: 2.0,
            downloaded_bytes: 0,
            total_bytes: 0,
            speed_bytes_per_sec: 0,
            eta_seconds: None,
            active_segments: vec![],
            error: None,
        });

        let video_probe_fut = probe_media_stream(client, video_url, headers);
        let audio_probe_fut = async {
            match audio_url {
                Some(a_url) if a_url != video_url => {
                    Ok::<Option<ProbeResult>, String>(Some(
                        probe_media_stream(client, a_url, headers).await?,
                    ))
                }
                _ => Ok(None),
            }
        };

        let (v_probe_res, a_probe_res) = tokio::join!(video_probe_fut, audio_probe_fut);
        let v_probe = v_probe_res?;
        let a_probe = a_probe_res?;

        let v_total = match &v_probe {
            ProbeResult::Partial { total_len, .. } => *total_len,
            ProbeResult::FullStream(resp) => resp
                .content_length()
                .or_else(|| parse_clen_from_url(video_url))
                .unwrap_or(0),
        };
        let a_total = match &a_probe {
            Some(ProbeResult::Partial { total_len, .. }) => *total_len,
            Some(ProbeResult::FullStream(resp)) => resp
                .content_length()
                .or_else(|| audio_url.and_then(parse_clen_from_url))
                .unwrap_or(0),
            None => 0,
        };

        info!(
            video_expected_bytes = v_total,
            audio_expected_bytes = a_total,
            "Probed split A/V stream lengths"
        );

        let mut seg_plans: Vec<SegmentProgress> = Vec::new();
        let mut next_seg_id = 0usize;

        enum StreamPlan {
            Full {
                seg_id: usize,
                expected_hint: u64,
                resp: reqwest::Response,
            },
            Ranged {
                total_len: u64,
                first_byte: Option<u8>,
                ranges: Vec<(usize, u64, u64)>, // (seg_id, start, end)
            },
        }

        let build_plan = |probe: ProbeResult,
                          expected_hint: u64,
                          base_offset: u64,
                          is_video: bool,
                          next_id: &mut usize,
                          plans: &mut Vec<SegmentProgress>|
         -> StreamPlan {
            match probe {
                ProbeResult::FullStream(resp) => {
                    let len = resp.content_length().filter(|&c| c > 0).unwrap_or(expected_hint);
                    let id = *next_id;
                    *next_id += 1;
                    plans.push(SegmentProgress {
                        id,
                        start_byte: base_offset,
                        end_byte: base_offset + len.saturating_sub(1),
                        current_byte: base_offset,
                        downloaded: 0,
                        speed: 0,
                        status: SegmentStatus::Downloading,
                    });
                    StreamPlan::Full {
                        seg_id: id,
                        expected_hint: len,
                        resp,
                    }
                }
                ProbeResult::Partial {
                    total_len,
                    first_byte,
                } => {
                    let conns = if total_len < 512 * 1024 {
                        1usize
                    } else if is_video {
                        if total_len > 12 * 1024 * 1024 {
                            6
                        } else {
                            4
                        }
                    } else {
                        2
                    };

                    if total_len == 0 {
                        let id = *next_id;
                        *next_id += 1;
                        plans.push(SegmentProgress {
                            id,
                            start_byte: base_offset,
                            end_byte: base_offset,
                            current_byte: base_offset,
                            downloaded: 0,
                            speed: 0,
                            status: SegmentStatus::Downloading,
                        });
                        return StreamPlan::Ranged {
                            total_len: 0,
                            first_byte,
                            ranges: vec![(id, 0, 0)],
                        };
                    }

                    let part = total_len / (conns as u64);
                    let mut ranges = Vec::with_capacity(conns);
                    for i in 0..conns {
                        let s = (i as u64) * part;
                        let e = if i == conns - 1 {
                            total_len - 1
                        } else {
                            ((i + 1) as u64) * part - 1
                        };
                        let id = *next_id;
                        *next_id += 1;
                        let init_dl = if i == 0 && first_byte.is_some() { 1 } else { 0 };
                        plans.push(SegmentProgress {
                            id,
                            start_byte: base_offset + s,
                            end_byte: base_offset + e,
                            current_byte: base_offset + s + init_dl,
                            downloaded: init_dl,
                            speed: 0,
                            status: if init_dl == (e - s + 1) {
                                SegmentStatus::Completed
                            } else {
                                SegmentStatus::Downloading
                            },
                        });
                        ranges.push((id, s, e));
                    }
                    StreamPlan::Ranged {
                        total_len,
                        first_byte,
                        ranges,
                    }
                }
            }
        };

        let video_plan = build_plan(v_probe, v_total, 0, true, &mut next_seg_id, &mut seg_plans);
        let audio_plan = a_probe.map(|ap| {
            build_plan(
                ap,
                a_total,
                v_total,
                false,
                &mut next_seg_id,
                &mut seg_plans,
            )
        });

        let initial_dl: u64 = seg_plans.iter().map(|s| s.downloaded).sum();
        let dl_counter = Arc::new(AtomicU64::new(initial_dl));
        let tot_counter = Arc::new(AtomicU64::new(v_total + a_total));
        let segments_state = Arc::new(Mutex::new(seg_plans.clone()));
        let (tx, mut rx) = mpsc::unbounded_channel::<()>();

        let total_segs_count = seg_plans.len().max(1);
        progress_cb(MuxProgress {
            phase: "downloading".to_string(),
            segments_done: 0,
            segments_total: total_segs_count,
            percent: 3.0,
            downloaded_bytes: initial_dl,
            total_bytes: v_total + a_total,
            speed_bytes_per_sec: 0,
            eta_seconds: None,
            active_segments: seg_plans,
            error: None,
        });

        let execute_plan = |url: String,
                            plan: StreamPlan,
                            expected_total: u64,
                            tx_c: mpsc::UnboundedSender<()>,
                            dl_c: Arc<AtomicU64>,
                            tot_c: Arc<AtomicU64>,
                            segs_c: Arc<Mutex<Vec<SegmentProgress>>>| async move {
            match plan {
                StreamPlan::Full {
                    seg_id,
                    expected_hint,
                    resp,
                } => {
                    let bytes = consume_full_response_with_resume(
                        client,
                        &url,
                        headers,
                        expected_hint.max(expected_total),
                        resp,
                        seg_id,
                        dl_c,
                        tot_c,
                        segs_c,
                        tx_c,
                    )
                    .await?;
                    Ok::<Bytes, String>(Bytes::from(bytes))
                }
                StreamPlan::Ranged {
                    total_len,
                    first_byte,
                    ranges,
                } => {
                    if total_len == 1 && first_byte.is_some() {
                        return Ok(Bytes::from(vec![first_byte.unwrap()]));
                    }
                    if total_len == 0 {
                        let resp = send_request_with_headers(
                            client,
                            reqwest::Method::GET,
                            &url,
                            headers,
                            None,
                        )
                        .await
                        .map_err(|e| format!("Stream request failed: {}", e))?;
                        if !resp.status().is_success() {
                            return Err(format!("Media stream returned HTTP {}", resp.status()));
                        }
                        let bytes = consume_full_response_with_resume(
                            client,
                            &url,
                            headers,
                            expected_total,
                            resp,
                            ranges[0].0,
                            dl_c,
                            tot_c,
                            segs_c,
                            tx_c,
                        )
                        .await?;
                        return Ok(Bytes::from(bytes));
                    }

                    let futs = ranges.into_iter().enumerate().map(|(idx, (seg_id, s, e))| {
                        let url_ref = url.clone();
                        let hdr_clone = headers.cloned();
                        let dl_ref = Arc::clone(&dl_c);
                        let segs_ref = Arc::clone(&segs_c);
                        let tx_ref = tx_c.clone();
                        let fb = if idx == 0 { first_byte } else { None };
                        async move {
                            if let Some(b0) = fb {
                                if s == e {
                                    return Ok(vec![b0]);
                                }
                                let mut part = fetch_range_with_retry(
                                    client,
                                    &url_ref,
                                    hdr_clone.as_ref(),
                                    s + 1,
                                    e,
                                    seg_id,
                                    dl_ref,
                                    segs_ref,
                                    tx_ref,
                                )
                                .await?;
                                let mut full = Vec::with_capacity(1 + part.len());
                                full.push(b0);
                                full.append(&mut part);
                                Ok(full)
                            } else {
                                fetch_range_with_retry(
                                    client,
                                    &url_ref,
                                    hdr_clone.as_ref(),
                                    s,
                                    e,
                                    seg_id,
                                    dl_ref,
                                    segs_ref,
                                    tx_ref,
                                )
                                .await
                            }
                        }
                    });

                    let results = futures_util::future::join_all(futs).await;
                    let mut owned_parts = Vec::with_capacity(results.len());
                    for res in results {
                        owned_parts.push(res?);
                    }
                    Ok(concat_owned_chunks(owned_parts, total_len as usize))
                }
            }
        };

        let tx_video = tx.clone();
        let dl_v = Arc::clone(&dl_counter);
        let tot_v = Arc::clone(&tot_counter);
        let segs_v = Arc::clone(&segments_state);
        let video_url_owned = video_url.to_string();
        let video_fut = async move {
            let _keep = &tx_video;
            execute_plan(
                video_url_owned,
                video_plan,
                v_total,
                tx_video.clone(),
                dl_v,
                tot_v,
                segs_v,
            )
            .await
        };

        let tx_audio = tx.clone();
        let dl_a = Arc::clone(&dl_counter);
        let tot_a = Arc::clone(&tot_counter);
        let segs_a = Arc::clone(&segments_state);
        let audio_url_owned = audio_url.map(|s| s.to_string());
        let audio_fut = async move {
            let _keep = &tx_audio;
            match (audio_url_owned, audio_plan) {
                (Some(a_url), Some(plan)) => {
                    let bytes = execute_plan(
                        a_url,
                        plan,
                        a_total,
                        tx_audio.clone(),
                        dl_a,
                        tot_a,
                        segs_a,
                    )
                    .await?;
                    Ok::<Option<Bytes>, String>(Some(bytes))
                }
                _ => Ok(None),
            }
        };
        drop(tx);

        let fetch_both = async { tokio::join!(video_fut, audio_fut) };
        tokio::pin!(fetch_both);
        let mut rx_closed = false;
        let mut speed_window_start = Instant::now();
        let mut last_dl_bytes = initial_dl;
        let mut smoothed_speed: f64 = 0.0;

        let (video_res, audio_res) = loop {
            tokio::select! {
                res = &mut fetch_both => break res,
                maybe_prog = rx.recv(), if !rx_closed => {
                    match maybe_prog {
                        Some(()) => {
                            let elapsed = speed_window_start.elapsed().as_secs_f64();
                            if elapsed >= 0.15 {
                                let dl = dl_counter.load(Ordering::Relaxed);
                                let tot = tot_counter.load(Ordering::Relaxed).max(dl);
                                let delta = dl.saturating_sub(last_dl_bytes);
                                let inst_speed = (delta as f64) / elapsed;
                                smoothed_speed = if smoothed_speed <= 0.0 {
                                    inst_speed
                                } else {
                                    smoothed_speed * 0.6 + inst_speed * 0.4
                                };
                                let speed_u64 = smoothed_speed.max(0.0) as u64;
                                last_dl_bytes = dl;
                                speed_window_start = Instant::now();

                                let remaining = tot.saturating_sub(dl);
                                let eta = if speed_u64 > 0 {
                                    Some(remaining / speed_u64)
                                } else {
                                    None
                                };
                                let seg_snap = segments_state.lock().clone();
                                let done_cnt = seg_snap
                                    .iter()
                                    .filter(|s| s.status == SegmentStatus::Completed)
                                    .count();
                                let pct = if tot > 0 {
                                    3.0 + ((dl as f64 / tot as f64).min(1.0)) * 82.0
                                } else {
                                    35.0
                                };

                                progress_cb(MuxProgress {
                                    phase: "downloading".to_string(),
                                    segments_done: done_cnt,
                                    segments_total: seg_snap.len().max(1),
                                    percent: pct,
                                    downloaded_bytes: dl,
                                    total_bytes: tot,
                                    speed_bytes_per_sec: speed_u64,
                                    eta_seconds: eta,
                                    active_segments: seg_snap,
                                    error: None,
                                });
                            }
                        }
                        None => {
                            rx_closed = true;
                        }
                    }
                }
            }
        };

        let video_bytes = video_res?;
        let audio_bytes_opt = audio_res?;

        if v_total > 0 && (video_bytes.len() as u64) != v_total {
            return Err(format!(
                "Downloaded video stream size mismatch: got {} bytes, expected {} bytes",
                video_bytes.len(),
                v_total
            ));
        }
        if let Some(ref ab) = audio_bytes_opt {
            if a_total > 0 && (ab.len() as u64) != a_total {
                return Err(format!(
                    "Downloaded audio stream size mismatch: got {} bytes, expected {} bytes",
                    ab.len(),
                    a_total
                ));
            }
        }

        validate_media_payload(&video_bytes, "Video")?;
        if let Some(ref ab) = audio_bytes_opt {
            validate_media_payload(ab, "Audio")?;
        }

        let final_dl = (video_bytes.len()
            + audio_bytes_opt.as_ref().map(|a| a.len()).unwrap_or(0)) as u64;
        let final_segs = segments_state.lock().clone();
        let seg_count = final_segs.len().max(1);
        progress_cb(MuxProgress {
            phase: "muxing".to_string(),
            segments_done: seg_count,
            segments_total: seg_count,
            percent: 90.0,
            downloaded_bytes: final_dl,
            total_bytes: final_dl,
            speed_bytes_per_sec: 0,
            eta_seconds: Some(1),
            active_segments: final_segs.clone(),
            error: None,
        });

        let dest_owned = destination_path.to_path_buf();
        let mux_res = tokio::task::spawn_blocking(move || -> Result<(PathBuf, u64), String> {
            let v_len = video_bytes.len();
            let a_len = audio_bytes_opt.as_ref().map(|a| a.len()).unwrap_or(0);
            let video_track = parse_media_samples_bytes(video_bytes)?;
            let audio_track = match audio_bytes_opt {
                Some(ab) => Some(parse_media_samples_bytes(ab)?),
                None => None,
            };

            let container = select_output_container(&video_track, audio_track.as_ref())?;
            info!(
                video_codec = video_track.codec.as_display_name(),
                audio_codec = audio_track.as_ref().map(|a| a.codec.as_display_name()).unwrap_or("None"),
                video_samples = video_track.samples.len(),
                audio_samples = audio_track.as_ref().map(|a| a.samples.len()).unwrap_or(0),
                resolution = %format!("{}x{}", video_track.width, video_track.height),
                input_bytes = v_len + a_len,
                container = container.extension(),
                "Muxing split media tracks into adaptive container"
            );

            write_muxed_container(&video_track, audio_track.as_ref(), &dest_owned)
        })
        .await
        .map_err(|e| format!("Muxer worker join error: {}", e))?;

        let (out_file_path, final_size) = match mux_res {
            Ok(ok) => ok,
            Err(e) => {
                error!(error = %e, "Pure-Rust muxing or container verification failed");
                return Err(e);
            }
        };

        info!(
            output = %out_file_path.display(),
            input_bytes = final_dl,
            final_size,
            "Completed split A/V download, mux, and structural container verification"
        );

        progress_cb(MuxProgress {
            phase: "completed".to_string(),
            segments_done: seg_count,
            segments_total: seg_count,
            percent: 100.0,
            downloaded_bytes: final_size,
            total_bytes: final_size,
            speed_bytes_per_sec: 0,
            eta_seconds: Some(0),
            active_segments: final_segs,
            error: None,
        });

        Ok((out_file_path, final_size))
    }

    /// Inspect an HLS (.m3u8) or DASH (.mpd) manifest
    pub async fn inspect_manifest(
        client: &reqwest::Client,
        url_str: &str,
    ) -> Result<MediaManifestInfo, String> {
        let base_url = Url::parse(url_str).map_err(|e| format!("Invalid manifest URL: {}", e))?;
        let resp = client
            .get(url_str)
            .send()
            .await
            .map_err(|e| format!("Failed to fetch manifest: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("Manifest returned HTTP {}", resp.status()));
        }

        let body = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read manifest body: {}", e))?;

        if url_str.contains(".mpd") || body.contains("<MPD") {
            Self::parse_dash_manifest(&base_url, &body)
        } else {
            Self::parse_hls_manifest(client, &base_url, &body).await
        }
    }

    /// Parse HLS Master / Media playlist
    pub async fn parse_hls_manifest(
        client: &reqwest::Client,
        base_url: &Url,
        body: &str,
    ) -> Result<MediaManifestInfo, String> {
        if !body.contains("#EXTM3U") {
            return Err("Invalid HLS playlist: missing #EXTM3U header".to_string());
        }

        let mut video_tracks = Vec::new();
        let mut audio_tracks = Vec::new();
        let mut total_duration = 0.0f64;

        if body.contains("#EXT-X-STREAM-INF") {
            let mut current_bw = 0u64;
            let mut current_res = None;
            let mut current_codecs = "avc1".to_string();

            for line in body.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("#EXT-X-STREAM-INF:") {
                    current_bw = Self::extract_attribute(trimmed, "BANDWIDTH")
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(1000000);
                    current_res = Self::extract_attribute(trimmed, "RESOLUTION");
                    if let Some(c) = Self::extract_attribute(trimmed, "CODECS") {
                        current_codecs = c;
                    }
                } else if trimmed.starts_with("#EXT-X-MEDIA:TYPE=AUDIO") {
                    if let Some(uri) = Self::extract_attribute(trimmed, "URI") {
                        let audio_url = base_url.join(&uri).unwrap_or_else(|_| base_url.clone());
                        audio_tracks.push(MediaTrackInfo {
                            track_type: "audio".to_string(),
                            codec: "mp4a".to_string(),
                            resolution: None,
                            bandwidth: 128000,
                            init_url: None,
                            segment_urls: vec![audio_url.to_string()],
                        });
                    }
                } else if !trimmed.starts_with('#') && !trimmed.is_empty() {
                    let media_url = base_url.join(trimmed).unwrap_or_else(|_| base_url.clone());
                    video_tracks.push(MediaTrackInfo {
                        track_type: "video".to_string(),
                        codec: current_codecs.clone(),
                        resolution: current_res.clone(),
                        bandwidth: current_bw,
                        init_url: None,
                        segment_urls: vec![media_url.to_string()],
                    });
                    current_bw = 0;
                    current_res = None;
                }
            }

            if let Some(best_variant) = video_tracks.iter_mut().max_by_key(|v| v.bandwidth) {
                if let Some(variant_url) = best_variant.segment_urls.first().cloned() {
                    if let Ok(v_base) = Url::parse(&variant_url) {
                        if let Ok(v_resp) = client.get(&variant_url).send().await {
                            if let Ok(v_body) = v_resp.text().await {
                                let (init, segs, dur) = Self::parse_hls_segments(&v_base, &v_body);
                                best_variant.init_url = init;
                                best_variant.segment_urls = segs;
                                total_duration = dur;
                            }
                        }
                    }
                }
            }
        } else {
            let (init, segs, dur) = Self::parse_hls_segments(base_url, body);
            total_duration = dur;
            video_tracks.push(MediaTrackInfo {
                track_type: "video".to_string(),
                codec: "avc1".to_string(),
                resolution: None,
                bandwidth: 2000000,
                init_url: init,
                segment_urls: segs,
            });
        }

        let recommended_res = video_tracks.first().and_then(|v| v.resolution.clone());
        let recommended_audio_br = Some(192);

        Ok(MediaManifestInfo {
            protocol: "HLS".to_string(),
            video_tracks,
            audio_tracks,
            total_duration_secs: total_duration,
            recommended_video_resolution: recommended_res,
            recommended_audio_bitrate: recommended_audio_br,
        })
    }

    fn parse_hls_segments(base_url: &Url, body: &str) -> (Option<String>, Vec<String>, f64) {
        let mut init_url = None;
        let mut segments = Vec::new();
        let mut total_duration = 0.0f64;

        for line in body.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("#EXT-X-MAP:") {
                if let Some(uri) = Self::extract_attribute(trimmed, "URI") {
                    if let Ok(u) = base_url.join(&uri) {
                        init_url = Some(u.to_string());
                    }
                }
            } else if trimmed.starts_with("#EXTINF:") {
                let dur_str = trimmed
                    .trim_start_matches("#EXTINF:")
                    .split(',')
                    .next()
                    .unwrap_or("0");
                if let Ok(d) = dur_str.parse::<f64>() {
                    total_duration += d;
                }
            } else if !trimmed.starts_with('#') && !trimmed.is_empty() {
                if let Ok(u) = base_url.join(trimmed) {
                    segments.push(u.to_string());
                }
            }
        }

        (init_url, segments, total_duration)
    }

    /// Parse DASH MPD XML manifest
    pub fn parse_dash_manifest(base_url: &Url, body: &str) -> Result<MediaManifestInfo, String> {
        let mut video_tracks = Vec::new();
        let mut audio_tracks = Vec::new();

        let mut in_video_set = false;
        let mut in_audio_set = false;
        let mut current_template_init = None;
        let mut current_template_media = None;
        let mut current_start_number = 1usize;
        let mut current_bandwidth = 0u64;
        let mut current_width = 0u16;
        let mut current_height = 0u16;
        let mut current_rep_id = String::new();
        let mut current_codec = String::new();

        for line in body.lines() {
            let l = line.trim();
            if l.contains("<AdaptationSet") {
                in_video_set = l.contains("video") || l.contains("mimeType=\"video/mp4\"");
                in_audio_set = l.contains("audio") || l.contains("mimeType=\"audio/mp4\"");
            } else if l.contains("</AdaptationSet>") {
                in_video_set = false;
                in_audio_set = false;
            }

            if l.contains("<SegmentTemplate") {
                current_template_init = Self::extract_xml_attr(l, "initialization");
                current_template_media = Self::extract_xml_attr(l, "media");
                if let Some(sn) =
                    Self::extract_xml_attr(l, "startNumber").and_then(|s| s.parse().ok())
                {
                    current_start_number = sn;
                }
            }

            if l.contains("<Representation") {
                if let Some(id) = Self::extract_xml_attr(l, "id") {
                    current_rep_id = id;
                }
                if let Some(bw) = Self::extract_xml_attr(l, "bandwidth").and_then(|s| s.parse().ok())
                {
                    current_bandwidth = bw;
                }
                if let Some(w) = Self::extract_xml_attr(l, "width").and_then(|s| s.parse().ok()) {
                    current_width = w;
                }
                if let Some(h) = Self::extract_xml_attr(l, "height").and_then(|s| s.parse().ok())
                {
                    current_height = h;
                }
                if let Some(c) = Self::extract_xml_attr(l, "codecs") {
                    current_codec = c;
                }

                let init_url = current_template_init.as_ref().map(|tpl| {
                    let expanded = tpl.replace("$RepresentationID$", &current_rep_id);
                    base_url
                        .join(&expanded)
                        .map(|u| u.to_string())
                        .unwrap_or(expanded)
                });

                let mut seg_urls = Vec::new();
                if let Some(ref tpl) = current_template_media {
                    for i in 0..10 {
                        let seg_num = current_start_number + i;
                        let expanded = tpl
                            .replace("$RepresentationID$", &current_rep_id)
                            .replace("$Number$", &seg_num.to_string())
                            .replace("$Number%05d$", &format!("{:05}", seg_num));
                        if let Ok(u) = base_url.join(&expanded) {
                            seg_urls.push(u.to_string());
                        }
                    }
                }

                if in_video_set
                    || current_codec.contains("avc")
                    || current_codec.contains("hvc")
                    || current_width > 0
                {
                    video_tracks.push(MediaTrackInfo {
                        track_type: "video".to_string(),
                        codec: if current_codec.is_empty() {
                            "avc1".to_string()
                        } else {
                            current_codec.clone()
                        },
                        resolution: if current_width > 0 {
                            Some(format!("{}x{}", current_width, current_height))
                        } else {
                            None
                        },
                        bandwidth: current_bandwidth,
                        init_url,
                        segment_urls: seg_urls,
                    });
                } else if in_audio_set || current_codec.contains("mp4a") {
                    audio_tracks.push(MediaTrackInfo {
                        track_type: "audio".to_string(),
                        codec: if current_codec.is_empty() {
                            "mp4a".to_string()
                        } else {
                            current_codec.clone()
                        },
                        resolution: None,
                        bandwidth: current_bandwidth,
                        init_url,
                        segment_urls: seg_urls,
                    });
                }
            }
        }

        let recommended_res = video_tracks
            .iter()
            .max_by_key(|v| v.bandwidth)
            .and_then(|v| v.resolution.clone());
        let recommended_audio_br = audio_tracks
            .first()
            .map(|a| (a.bandwidth / 1000) as u32)
            .or(Some(128));

        Ok(MediaManifestInfo {
            protocol: "DASH".to_string(),
            video_tracks,
            audio_tracks,
            total_duration_secs: 60.0,
            recommended_video_resolution: recommended_res,
            recommended_audio_bitrate: recommended_audio_br,
        })
    }

    /// Execute parallel stream download and in-memory native remuxing to .mp4
    pub async fn download_and_remux<F>(
        client: &reqwest::Client,
        manifest_url: &str,
        destination_path: &Path,
        mut progress_cb: F,
    ) -> Result<PathBuf, String>
    where
        F: FnMut(MuxProgress) + Send + 'static,
    {
        progress_cb(MuxProgress {
            phase: "parsing".to_string(),
            segments_done: 0,
            segments_total: 0,
            percent: 5.0,
            downloaded_bytes: 0,
            total_bytes: 0,
            speed_bytes_per_sec: 0,
            eta_seconds: None,
            active_segments: vec![],
            error: None,
        });

        let manifest = Self::inspect_manifest(client, manifest_url).await?;
        let video_track = manifest
            .video_tracks
            .iter()
            .max_by_key(|t| t.bandwidth)
            .ok_or_else(|| "No video track found in manifest".to_string())?;

        let total_segments = video_track.segment_urls.len();
        if total_segments == 0 {
            return Err("Manifest contains 0 media segments".to_string());
        }

        progress_cb(MuxProgress {
            phase: "downloading".to_string(),
            segments_done: 0,
            segments_total: total_segments,
            percent: 10.0,
            downloaded_bytes: 0,
            total_bytes: 0,
            speed_bytes_per_sec: 0,
            eta_seconds: None,
            active_segments: vec![],
            error: None,
        });

        let mut init_video_bytes: Option<Vec<u8>> = None;
        if let Some(ref init_u) = video_track.init_url {
            if let Ok(resp) = client.get(init_u).send().await {
                if resp.status().is_success() {
                    if let Ok(bytes) = resp.bytes().await {
                        init_video_bytes = Some(bytes.to_vec());
                    }
                }
            }
        }

        let mut downloaded_buffers: Vec<Option<Vec<u8>>> = vec![None; total_segments];
        let concurrency = 6;
        let mut index = 0;
        let mut downloaded_sum: u64 = 0;

        while index < total_segments {
            let batch_end = (index + concurrency).min(total_segments);
            let mut handles = Vec::new();

            for i in index..batch_end {
                let seg_url = video_track.segment_urls[i].clone();
                let client_c = client.clone();
                handles.push(tokio::spawn(async move {
                    let resp = client_c.get(&seg_url).send().await.map_err(|e| e.to_string())?;
                    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
                    Ok::<(usize, Vec<u8>), String>((i, bytes.to_vec()))
                }));
            }

            for h in handles {
                match h.await {
                    Ok(Ok((i, buf))) => {
                        downloaded_sum += buf.len() as u64;
                        downloaded_buffers[i] = Some(buf);
                    }
                    Ok(Err(e)) => return Err(format!("Failed to fetch segment: {}", e)),
                    Err(e) => return Err(format!("Task execution error: {}", e)),
                }
            }

            index = batch_end;
            let done_count = downloaded_buffers.iter().filter(|b| b.is_some()).count();
            let dl_percent = 10.0 + (done_count as f64 / total_segments as f64) * 60.0;

            progress_cb(MuxProgress {
                phase: "downloading".to_string(),
                segments_done: done_count,
                segments_total: total_segments,
                percent: dl_percent,
                downloaded_bytes: downloaded_sum,
                total_bytes: downloaded_sum.max(1),
                speed_bytes_per_sec: 0,
                eta_seconds: None,
                active_segments: vec![],
                error: None,
            });
        }

        progress_cb(MuxProgress {
            phase: "muxing".to_string(),
            segments_done: total_segments,
            segments_total: total_segments,
            percent: 75.0,
            downloaded_bytes: downloaded_sum,
            total_bytes: downloaded_sum,
            speed_bytes_per_sec: 0,
            eta_seconds: None,
            active_segments: vec![],
            error: None,
        });

        let out_file_path = if destination_path.extension().is_none() {
            destination_path.with_extension("mp4")
        } else {
            destination_path.to_path_buf()
        };

        if let Some(parent) = out_file_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let first_chunk = downloaded_buffers
            .iter()
            .flatten()
            .next()
            .ok_or_else(|| "No segment buffers available".to_string())?;

        let is_mpeg_ts = first_chunk.first() == Some(&0x47);

        let mut audio_buffers: Vec<Option<Vec<u8>>> = Vec::new();
        if let Some(audio_track) = manifest.audio_tracks.iter().max_by_key(|t| t.bandwidth) {
            if let Some(ref init_u) = audio_track.init_url {
                if let Ok(resp) = client.get(init_u).send().await {
                    if resp.status().is_success() {
                        if let Ok(bytes) = resp.bytes().await {
                            audio_buffers.push(Some(bytes.to_vec()));
                        }
                    }
                }
            }
            for seg_url in &audio_track.segment_urls {
                if let Ok(resp) = client.get(seg_url).send().await {
                    if let Ok(bytes) = resp.bytes().await {
                        audio_buffers.push(Some(bytes.to_vec()));
                    }
                }
            }
        }

        if let Some(init_v) = init_video_bytes {
            downloaded_buffers.insert(0, Some(init_v));
        }

        if is_mpeg_ts {
            Self::remux_ts_to_mp4(&downloaded_buffers, &audio_buffers, &out_file_path)?;
        } else {
            Self::remux_fmp4_to_mp4(
                video_track.init_url.as_deref(),
                &downloaded_buffers,
                &audio_buffers,
                &out_file_path,
            )?;
        }

        progress_cb(MuxProgress {
            phase: "completed".to_string(),
            segments_done: total_segments,
            segments_total: total_segments,
            percent: 100.0,
            downloaded_bytes: downloaded_sum,
            total_bytes: downloaded_sum,
            speed_bytes_per_sec: 0,
            eta_seconds: Some(0),
            active_segments: vec![],
            error: None,
        });

        Ok(out_file_path)
    }

    /// Remux MPEG-TS segment packets into standard MP4 container
    fn remux_ts_to_mp4(
        buffers: &[Option<Vec<u8>>],
        audio_buffers: &[Option<Vec<u8>>],
        output_path: &Path,
    ) -> Result<(), String> {
        let file = File::create(output_path)
            .map_err(|e| format!("Failed to create output file: {}", e))?;
        let mut buf_writer = BufWriter::new(file);

        let mp4_config = Mp4Config {
            major_brand: str::parse("isom").unwrap(),
            minor_version: 512,
            compatible_brands: vec![
                str::parse("isom").unwrap(),
                str::parse("iso2").unwrap(),
                str::parse("avc1").unwrap(),
                str::parse("mp41").unwrap(),
            ],
            timescale: 1000,
        };

        let mut writer = Mp4Writer::write_start(&mut buf_writer, &mp4_config)
            .map_err(|e| format!("Mp4Writer init error: {}", e))?;

        let avc_config = AvcConfig {
            width: 1920,
            height: 1080,
            seq_param_set: vec![
                0x67, 0x42, 0xc0, 0x28, 0xd9, 0x00, 0x78, 0x02, 0x27, 0xe5, 0x84, 0x00, 0x00, 0x03,
                0x00, 0x04, 0x00, 0x00, 0x03, 0x00, 0xf0, 0x3c, 0x60, 0xc9, 0x20,
            ],
            pic_param_set: vec![0x68, 0xcb, 0x8c, 0xb2],
        };
        writer
            .add_track(&TrackConfig::from(avc_config))
            .map_err(|e| format!("Failed to add video track: {}", e))?;

        if !audio_buffers.is_empty() {
            let aac_config = AacConfig {
                bitrate: 192000,
                profile: AudioObjectType::AacLowComplexity,
                freq_index: SampleFreqIndex::Freq48000,
                chan_conf: ChannelConfig::Stereo,
            };
            writer
                .add_track(&TrackConfig::from(aac_config))
                .map_err(|e| format!("Failed to add audio track: {}", e))?;
        }

        let mut video_time = 0u64;
        let mut audio_time = 0u64;
        for (seg_idx, buf_opt) in buffers.iter().enumerate() {
            if let Some(ref buf) = buf_opt {
                let chunks = buf.chunks_exact(188);
                let mut pes_payload = Vec::new();

                for packet in chunks {
                    let payload_unit_start = (packet[1] & 0x40) != 0;
                    let has_adaptation = (packet[3] & 0x20) != 0;
                    let has_payload = (packet[3] & 0x10) != 0;

                    let mut offset = 4;
                    if has_adaptation {
                        let adapt_len = packet[4] as usize;
                        offset += 1 + adapt_len;
                    }

                    if has_payload && offset < 188 {
                        if payload_unit_start && !pes_payload.is_empty() {
                            let sample = Mp4Sample {
                                start_time: video_time,
                                duration: 40,
                                rendering_offset: 0,
                                is_sync: seg_idx == 0,
                                bytes: Bytes::from(pes_payload.clone()),
                            };
                            let _ = writer.write_sample(1, &sample);
                            video_time += 40;
                            pes_payload.clear();
                        }
                        pes_payload.extend_from_slice(&packet[offset..188]);
                    }
                }

                if !pes_payload.is_empty() {
                    let sample = Mp4Sample {
                        start_time: video_time,
                        duration: 40,
                        rendering_offset: 0,
                        is_sync: false,
                        bytes: Bytes::from(pes_payload),
                    };
                    let _ = writer.write_sample(1, &sample);
                    video_time += 40;
                }
            }

            if let Some(Some(ref aud_buf)) = audio_buffers.get(seg_idx) {
                let sample = Mp4Sample {
                    start_time: audio_time,
                    duration: 1000,
                    rendering_offset: 0,
                    is_sync: true,
                    bytes: Bytes::from(aud_buf.clone()),
                };
                let _ = writer.write_sample(2, &sample);
                audio_time += 1000;
            }
        }

        writer
            .write_end()
            .map_err(|e| format!("Failed to finalize MP4: {}", e))?;
        buf_writer
            .flush()
            .map_err(|e| format!("Failed to flush output: {}", e))?;
        Ok(())
    }

    /// Remux fragmented ISOBMFF/WebM chunks into a codec-appropriate container (MP4, WebM, or MKV)
    pub fn remux_fmp4_to_mp4(
        _init_url: Option<&str>,
        buffers: &[Option<Vec<u8>>],
        audio_buffers: &[Option<Vec<u8>>],
        output_path: &Path,
    ) -> Result<(), String> {
        let video_blob = concat_chunk_buffers(buffers);
        let audio_blob = concat_chunk_buffers(audio_buffers);

        let video_track = parse_media_samples(&video_blob)?;
        let audio_track = if !audio_blob.is_empty() {
            Some(parse_media_samples(&audio_blob)?)
        } else {
            None
        };

        write_muxed_container(&video_track, audio_track.as_ref(), output_path)?;
        Ok(())
    }

    fn extract_attribute(line: &str, attr: &str) -> Option<String> {
        let pattern = format!("{attr}=");
        if let Some(pos) = line.find(&pattern) {
            let rest = &line[pos + pattern.len()..];
            if rest.starts_with('"') {
                rest[1..].split('"').next().map(|s| s.to_string())
            } else {
                rest.split(&[',', ' ', '\n', '\r'][..])
                    .next()
                    .map(|s| s.to_string())
            }
        } else {
            None
        }
    }

    fn extract_xml_attr(line: &str, attr: &str) -> Option<String> {
        let pattern = format!("{attr}=\"");
        if let Some(pos) = line.find(&pattern) {
            let rest = &line[pos + pattern.len()..];
            rest.split('"').next().map(|s| s.to_string())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use uuid::Uuid;

    fn make_box(fourcc: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let len = (8 + payload.len()) as u32;
        let mut out = Vec::with_capacity(len as usize);
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(fourcc);
        out.extend_from_slice(payload);
        out
    }

    fn make_full_box(fourcc: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
        let mut payload = Vec::with_capacity(4 + body.len());
        payload.push(version);
        payload.extend_from_slice(&flags.to_be_bytes()[1..4]);
        payload.extend_from_slice(body);
        make_box(fourcc, &payload)
    }

    /// Build a synthetic fragmented MP4 track buffer (`ftyp` + `moov`(`mvex`/`trex`) + `moof`(`mfhd`/`traf`(`tfhd`/`tfdt`/`trun`)) + `mdat`)
    fn build_synthetic_fmp4(
        track_id: u32,
        timescale: u32,
        is_video: bool,
        samples: &[(&[u8], u32, bool)],
    ) -> Vec<u8> {
        let ftyp = make_box(b"ftyp", b"iso5\x00\x00\x02\x00iso5iso6mp41");

        // tkhd version 0: 88 bytes body -> track_id at body[8..12] (payload[12..16])
        let mut tkhd_body = vec![0u8; 88];
        tkhd_body[8..12].copy_from_slice(&track_id.to_be_bytes());
        if is_video {
            tkhd_body[72..76].copy_from_slice(&((1280u32 << 16).to_be_bytes()));
            tkhd_body[76..80].copy_from_slice(&((720u32 << 16).to_be_bytes()));
        }
        let tkhd = make_full_box(b"tkhd", 0, 7, &tkhd_body);

        // mdhd version 0: creation(4) + mod(4) + timescale(4) + dur(4) + lang(2) + pre(2) = 20 bytes
        let mut mdhd_body = vec![0u8; 20];
        mdhd_body[8..12].copy_from_slice(&timescale.to_be_bytes());
        // language "und" -> (21<<10)|(14<<5)|4 = 0x55C4
        mdhd_body[16..18].copy_from_slice(&0x55C4u16.to_be_bytes());
        let mdhd = make_full_box(b"mdhd", 0, 0, &mdhd_body);

        let sample_entry = if is_video {
            let sps = vec![
                0x67, 0x64, 0x00, 0x1F, 0xAC, 0xD9, 0x40, 0x50, 0x05, 0xBB, 0x01, 0x10, 0x00, 0x00,
                0x03, 0x00, 0x10, 0x00, 0x00, 0x03, 0x03, 0xC8, 0xF1, 0x83, 0x19, 0x60,
            ];
            let pps = vec![0x68, 0xEB, 0xE3, 0xCB, 0x22, 0xC0];
            let mut avcc = vec![0x01, sps[1], sps[2], sps[3], 0xFF, 0xE1];
            avcc.extend_from_slice(&(sps.len() as u16).to_be_bytes());
            avcc.extend_from_slice(&sps);
            avcc.push(1);
            avcc.extend_from_slice(&(pps.len() as u16).to_be_bytes());
            avcc.extend_from_slice(&pps);
            let avcc_box = make_box(b"avcC", &avcc);

            let mut avc1_payload = vec![0u8; 78];
            avc1_payload[24..26].copy_from_slice(&1280u16.to_be_bytes());
            avc1_payload[26..28].copy_from_slice(&720u16.to_be_bytes());
            avc1_payload.extend_from_slice(&avcc_box);
            make_box(b"avc1", &avc1_payload)
        } else {
            // esds: ES_Descr(0x03) -> DecConfigDescr(0x04) -> DecSpecificInfo(0x05) with AAC LC 48kHz stereo [0x11, 0x90]
            let dec_spec = vec![0x05, 0x02, 0x11, 0x90];
            let mut dec_cfg = vec![0x04, (13 + dec_spec.len()) as u8, 0x40, 0x15, 0, 0, 0];
            dec_cfg.extend_from_slice(&192_000u32.to_be_bytes()); // max bitrate
            dec_cfg.extend_from_slice(&128_000u32.to_be_bytes()); // avg bitrate
            dec_cfg.extend_from_slice(&dec_spec);

            let mut es_descr = vec![0x03, (3 + dec_cfg.len()) as u8, 0x00, 0x02, 0x00];
            es_descr.extend_from_slice(&dec_cfg);
            let esds_box = make_full_box(b"esds", 0, 0, &es_descr);

            let mut mp4a_payload = vec![0u8; 28];
            mp4a_payload[16..18].copy_from_slice(&2u16.to_be_bytes());
            mp4a_payload[18..20].copy_from_slice(&16u16.to_be_bytes());
            mp4a_payload[24..26].copy_from_slice(&48000u16.to_be_bytes());
            mp4a_payload.extend_from_slice(&esds_box);
            make_box(b"mp4a", &mp4a_payload)
        };

        let mut stsd_body = Vec::new();
        stsd_body.extend_from_slice(&1u32.to_be_bytes());
        stsd_body.extend_from_slice(&sample_entry);
        let stsd = make_full_box(b"stsd", 0, 0, &stsd_body);

        let stbl = make_box(b"stbl", &stsd);
        let minf = make_box(b"minf", &stbl);
        let mut mdia_payload = mdhd;
        mdia_payload.extend_from_slice(&minf);
        let mdia = make_box(b"mdia", &mdia_payload);

        let mut trak_payload = tkhd;
        trak_payload.extend_from_slice(&mdia);
        let trak = make_box(b"trak", &trak_payload);

        let mut trex_body = Vec::new();
        trex_body.extend_from_slice(&track_id.to_be_bytes());
        trex_body.extend_from_slice(&1u32.to_be_bytes());
        trex_body.extend_from_slice(&1000u32.to_be_bytes());
        trex_body.extend_from_slice(&0u32.to_be_bytes());
        trex_body.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        let trex = make_full_box(b"trex", 0, 0, &trex_body);
        let mvex = make_box(b"mvex", &trex);

        let mut moov_payload = trak;
        moov_payload.extend_from_slice(&mvex);
        let moov = make_box(b"moov", &moov_payload);

        // Fragment: moof + mdat
        let mut mdat_payload = Vec::new();
        for (bytes, _, _) in samples {
            mdat_payload.extend_from_slice(bytes);
        }
        let mdat = make_box(b"mdat", &mdat_payload);

        // tfhd with default-base-is-moof (0x020000)
        let tfhd = make_full_box(b"tfhd", 0, 0x020000, &track_id.to_be_bytes());
        let tfdt = make_full_box(b"tfdt", 0, 0, &0u32.to_be_bytes());

        // Compute trun size first so data_offset = moof_size + 8
        let trun_flags = 0x000001 | 0x000100 | 0x000200 | 0x000400; // data_offset, duration, size, flags
        let mut trun_body = Vec::new();
        trun_body.extend_from_slice(&(samples.len() as u32).to_be_bytes());
        let data_offset_idx = trun_body.len();
        trun_body.extend_from_slice(&0i32.to_be_bytes()); // placeholder
        for (bytes, dur, is_sync) in samples {
            trun_body.extend_from_slice(&dur.to_be_bytes());
            trun_body.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
            let s_flags: u32 = if *is_sync { 0x0200_0000 } else { 0x0101_0000 };
            trun_body.extend_from_slice(&s_flags.to_be_bytes());
        }

        let mfhd = make_full_box(b"mfhd", 0, 0, &1u32.to_be_bytes());
        let dummy_trun = make_full_box(b"trun", 0, trun_flags, &trun_body);
        let mut traf_payload = tfhd.clone();
        traf_payload.extend_from_slice(&tfdt);
        traf_payload.extend_from_slice(&dummy_trun);
        let traf = make_box(b"traf", &traf_payload);

        let mut moof_payload = mfhd.clone();
        moof_payload.extend_from_slice(&traf);
        let moof_total_size = (8 + moof_payload.len()) as i32;

        // Patch trun data_offset to point right after `mdat` 8-byte header
        trun_body[data_offset_idx..data_offset_idx + 4]
            .copy_from_slice(&(moof_total_size + 8).to_be_bytes());
        let real_trun = make_full_box(b"trun", 0, trun_flags, &trun_body);
        let mut real_traf_payload = tfhd;
        real_traf_payload.extend_from_slice(&tfdt);
        real_traf_payload.extend_from_slice(&real_trun);
        let real_traf = make_box(b"traf", &real_traf_payload);
        let mut real_moof_payload = mfhd;
        real_moof_payload.extend_from_slice(&real_traf);
        let moof = make_box(b"moof", &real_moof_payload);

        let mut out = Vec::new();
        out.extend_from_slice(&ftyp);
        out.extend_from_slice(&moov);
        out.extend_from_slice(&moof);
        out.extend_from_slice(&mdat);
        out
    }

    #[test]
    fn test_pure_rust_isobmff_fmp4_split_av_remux() {
        let vid_fmp4 = build_synthetic_fmp4(
            1,
            30_000,
            true,
            &[
                (b"\x00\x00\x00\x04\x65\x88\x84\x21", 1000, true),
                (b"\x00\x00\x00\x04\x41\x9A\x21\x00", 1000, false),
                (b"\x00\x00\x00\x04\x41\x9A\x42\x00", 1000, false),
            ],
        );
        let aud_fmp4 = build_synthetic_fmp4(
            2,
            48_000,
            false,
            &[
                (b"\x21\x10\x04\x60\x8C\x1C", 1024, true),
                (b"\x21\x10\x04\x60\x8C\x1D", 1024, true),
            ],
        );

        let parsed_v = parse_isobmff_samples(&vid_fmp4).expect("Video fMP4 parse failed");
        let parsed_a = parse_isobmff_samples(&aud_fmp4).expect("Audio fMP4 parse failed");

        assert_eq!(parsed_v.track_type, TrackType::Video);
        assert_eq!(parsed_v.timescale, 30_000);
        assert_eq!(parsed_v.samples.len(), 3);
        assert!(parsed_v.samples[0].is_sync);
        assert!(!parsed_v.samples[1].is_sync);
        assert_eq!(&parsed_v.samples[0].bytes[..], b"\x00\x00\x00\x04\x65\x88\x84\x21");

        assert_eq!(parsed_a.track_type, TrackType::Audio);
        assert_eq!(parsed_a.timescale, 48_000);
        assert_eq!(parsed_a.samples.len(), 2);

        let out_path = std::env::temp_dir().join(format!("octoheft_mux_{}.mp4", Uuid::new_v4()));
        let len = write_muxed_mp4(&parsed_v, Some(&parsed_a), &out_path).expect("write_muxed_mp4");
        assert!(len > 0);

        let file = File::open(&out_path).unwrap();
        let reader = Mp4Reader::read_header(file, len).expect("Valid readable MP4 expected");
        assert_eq!(reader.tracks().len(), 2, "Expected interleaved video + audio tracks");

        let t1 = reader.tracks().get(&1).expect("Track 1 missing");
        assert_eq!(t1.track_type().unwrap(), TrackType::Video);
        assert_eq!(t1.width(), 1280);
        assert_eq!(t1.height(), 720);
        assert_eq!(t1.sample_count(), 3);

        let t2 = reader.tracks().get(&2).expect("Track 2 missing");
        assert_eq!(t2.track_type().unwrap(), TrackType::Audio);
        assert_eq!(t2.sample_count(), 2);
        assert_eq!(t2.sample_freq_index().unwrap(), SampleFreqIndex::Freq48000);

        let _ = std::fs::remove_file(&out_path);
    }

    #[tokio::test]
    async fn test_download_and_remux_split_streams_http() {
        let vid_fmp4 = Arc::new(build_synthetic_fmp4(
            1,
            30_000,
            true,
            &[(b"\x00\x00\x00\x04\x65\x88\x84\x21", 1000, true)],
        ));
        let aud_fmp4 = Arc::new(build_synthetic_fmp4(
            1,
            48_000,
            false,
            &[(b"\x21\x10\x04\x60\x8C\x1C", 1024, true)],
        ));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let v_clone = Arc::clone(&vid_fmp4);
        let a_clone = Arc::clone(&aud_fmp4);
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let v = Arc::clone(&v_clone);
                let a = Arc::clone(&a_clone);
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let path = req.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("/");
                    let body = if path.contains("audio") { &a[..] } else { &v[..] };
                    let hdr = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: video/mp4\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = sock.write_all(hdr.as_bytes()).await;
                    let _ = sock.write_all(body).await;
                    let _ = sock.shutdown().await;
                });
            }
        });

        let video_url = format!("http://127.0.0.1:{}/videoplayback?mime=video/mp4", addr.port());
        let audio_url = format!("http://127.0.0.1:{}/videoplayback?mime=audio/mp4", addr.port());
        let dest = std::env::temp_dir().join(format!("octoheft_split_{}.mp4", Uuid::new_v4()));

        let client = reqwest::Client::new();
        let (final_path, final_len) = MediaMuxer::download_and_remux_split_streams(
            &client,
            &video_url,
            Some(&audio_url),
            None,
            &dest,
            |_| {},
        )
        .await
        .expect("Split stream download+mux failed");

        assert!(final_len > 0);
        let reader = Mp4Reader::read_header(File::open(&final_path).unwrap(), final_len)
            .expect("Muxed split stream MP4 should parse cleanly");
        assert_eq!(reader.tracks().len(), 2);

        let _ = std::fs::remove_file(&final_path);
    }

    fn build_synthetic_webm_track(
        doc_type: &str,
        codec_id: &str,
        is_video: bool,
        width: u16,
        height: u16,
        sample_rate: f64,
        channels: u16,
        codec_private: Option<&[u8]>,
        frames: &[(&[u8], i64, bool)],
    ) -> Vec<u8> {
        let ebml_hdr = make_ebml(&[0x1A, 0x45, 0xDF, 0xA3], &make_ebml(&[0x42, 0x82], doc_type.as_bytes()));
        let ts_scale = make_ebml(&[0x2A, 0xD7, 0xB1], &1_000_000u32.to_be_bytes());
        let info = make_ebml(&[0x15, 0x49, 0xA9, 0x66], &ts_scale);

        let mut track_entry_body = Vec::new();
        track_entry_body.extend_from_slice(&make_ebml(&[0xD7], &[1]));
        track_entry_body.extend_from_slice(&make_ebml(&[0x83], &[if is_video { 1 } else { 2 }]));
        track_entry_body.extend_from_slice(&make_ebml(&[0x86], codec_id.as_bytes()));
        if let Some(cp) = codec_private {
            track_entry_body.extend_from_slice(&make_ebml(&[0x63, 0xA2], cp));
        }
        if is_video {
            track_entry_body.extend_from_slice(&make_ebml(
                &[0x23, 0xE3, 0x83],
                &33_333_333u32.to_be_bytes(),
            ));
            let mut video_settings = Vec::new();
            video_settings.extend_from_slice(&make_ebml(&[0xB0], &width.to_be_bytes()));
            video_settings.extend_from_slice(&make_ebml(&[0xBA], &height.to_be_bytes()));
            track_entry_body.extend_from_slice(&make_ebml(&[0xE0], &video_settings));
        } else {
            track_entry_body.extend_from_slice(&make_ebml(
                &[0x56, 0xAA],
                &encode_ebml_uint(6_500_000),
            ));
            track_entry_body.extend_from_slice(&make_ebml(
                &[0x56, 0xBB],
                &encode_ebml_uint(80_000_000),
            ));
            let mut audio_settings = Vec::new();
            audio_settings.extend_from_slice(&make_ebml(&[0xB5], &sample_rate.to_be_bytes()));
            audio_settings.extend_from_slice(&make_ebml(&[0x9F], &encode_ebml_uint(channels as u64)));
            track_entry_body.extend_from_slice(&make_ebml(&[0xE1], &audio_settings));
        }
        let track_entry = make_ebml(&[0xAE], &track_entry_body);
        let tracks = make_ebml(&[0x16, 0x54, 0xAE, 0x6B], &track_entry);

        let mut seg_body = Vec::new();
        seg_body.extend_from_slice(&info);
        seg_body.extend_from_slice(&tracks);

        // Emit clusters grouped safely within i16 relative timestamps
        let mut idx = 0usize;
        while idx < frames.len() {
            let base_ts = frames[idx].1;
            let mut cluster_body = Vec::new();
            cluster_body.extend_from_slice(&make_ebml(&[0xE7], &encode_ebml_uint(base_ts.max(0) as u64)));
            while idx < frames.len() {
                let (payload, ts_ms, is_key) = frames[idx];
                let rel = ts_ms - base_ts;
                if rel > 30_000 || rel < -10_000 {
                    break;
                }
                let mut sb = Vec::with_capacity(4 + payload.len());
                sb.push(0x81);
                sb.extend_from_slice(&(rel as i16).to_be_bytes());
                sb.push(if is_key { 0x80 } else { 0x00 });
                sb.extend_from_slice(payload);
                cluster_body.extend_from_slice(&make_ebml(&[0xA3], &sb));
                idx += 1;
            }
            seg_body.extend_from_slice(&make_ebml(&[0x1F, 0x43, 0xB6, 0x75], &cluster_body));
        }

        let segment = make_ebml(&[0x18, 0x53, 0x80, 0x67], &seg_body);
        let mut out = ebml_hdr;
        out.extend_from_slice(&segment);
        out
    }

    fn build_synthetic_webm_vp9(
        width: u16,
        height: u16,
        frames: &[(&[u8], i16, bool)],
    ) -> Vec<u8> {
        let converted: Vec<(&[u8], i64, bool)> =
            frames.iter().map(|(p, r, k)| (*p, *r as i64, *k)).collect();
        build_synthetic_webm_track(
            "webm",
            "V_VP9",
            true,
            width,
            height,
            0.0,
            0,
            None,
            &converted,
        )
    }

    fn build_synthetic_fmp4_av1(
        track_id: u32,
        timescale: u32,
        width: u16,
        height: u16,
        samples: &[(&[u8], u32, bool)],
    ) -> Vec<u8> {
        let ftyp = make_box(b"ftyp", b"iso5\x00\x00\x02\x00iso5iso6mp41");

        let mut tkhd_body = vec![0u8; 88];
        tkhd_body[8..12].copy_from_slice(&track_id.to_be_bytes());
        tkhd_body[72..76].copy_from_slice(&(((width as u32) << 16).to_be_bytes()));
        tkhd_body[76..80].copy_from_slice(&(((height as u32) << 16).to_be_bytes()));
        let tkhd = make_full_box(b"tkhd", 0, 7, &tkhd_body);

        let mut mdhd_body = vec![0u8; 20];
        mdhd_body[8..12].copy_from_slice(&timescale.to_be_bytes());
        mdhd_body[16..18].copy_from_slice(&0x55C4u16.to_be_bytes());
        let mdhd = make_full_box(b"mdhd", 0, 0, &mdhd_body);

        let av1c_box = make_box(b"av1C", &[0x81, 0x0C, 0x0C, 0x00]);
        let mut av01_payload = vec![0u8; 78];
        av01_payload[24..26].copy_from_slice(&width.to_be_bytes());
        av01_payload[26..28].copy_from_slice(&height.to_be_bytes());
        av01_payload.extend_from_slice(&av1c_box);
        let sample_entry = make_box(b"av01", &av01_payload);

        let mut stsd_body = Vec::new();
        stsd_body.extend_from_slice(&1u32.to_be_bytes());
        stsd_body.extend_from_slice(&sample_entry);
        let stsd = make_full_box(b"stsd", 0, 0, &stsd_body);

        let stbl = make_box(b"stbl", &stsd);
        let minf = make_box(b"minf", &stbl);
        let mut mdia_payload = mdhd;
        mdia_payload.extend_from_slice(&minf);
        let mdia = make_box(b"mdia", &mdia_payload);
        let mut trak_payload = tkhd;
        trak_payload.extend_from_slice(&mdia);
        let trak = make_box(b"trak", &trak_payload);

        let mut trex_body = Vec::new();
        trex_body.extend_from_slice(&track_id.to_be_bytes());
        trex_body.extend_from_slice(&1u32.to_be_bytes());
        trex_body.extend_from_slice(&1000u32.to_be_bytes());
        trex_body.extend_from_slice(&0u32.to_be_bytes());
        trex_body.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        let trex = make_full_box(b"trex", 0, 0, &trex_body);
        let mvex = make_box(b"mvex", &trex);

        let mut moov_payload = trak;
        moov_payload.extend_from_slice(&mvex);
        let moov = make_box(b"moov", &moov_payload);

        let mut mdat_payload = Vec::new();
        for (bytes, _, _) in samples {
            mdat_payload.extend_from_slice(bytes);
        }
        let mdat = make_box(b"mdat", &mdat_payload);

        let tfhd = make_full_box(b"tfhd", 0, 0x020000, &track_id.to_be_bytes());
        let tfdt = make_full_box(b"tfdt", 0, 0, &0u32.to_be_bytes());
        let trun_flags = 0x000001 | 0x000100 | 0x000200 | 0x000400;
        let mut trun_body = Vec::new();
        trun_body.extend_from_slice(&(samples.len() as u32).to_be_bytes());
        let data_offset_idx = trun_body.len();
        trun_body.extend_from_slice(&0i32.to_be_bytes());
        for (bytes, dur, is_sync) in samples {
            trun_body.extend_from_slice(&dur.to_be_bytes());
            trun_body.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
            let s_flags: u32 = if *is_sync { 0x0200_0000 } else { 0x0101_0000 };
            trun_body.extend_from_slice(&s_flags.to_be_bytes());
        }
        let mfhd = make_full_box(b"mfhd", 0, 0, &1u32.to_be_bytes());
        let dummy_trun = make_full_box(b"trun", 0, trun_flags, &trun_body);
        let mut traf_payload = tfhd.clone();
        traf_payload.extend_from_slice(&tfdt);
        traf_payload.extend_from_slice(&dummy_trun);
        let traf = make_box(b"traf", &traf_payload);
        let mut moof_payload = mfhd.clone();
        moof_payload.extend_from_slice(&traf);
        let moof_total_size = (8 + moof_payload.len()) as i32;

        trun_body[data_offset_idx..data_offset_idx + 4]
            .copy_from_slice(&(moof_total_size + 8).to_be_bytes());
        let real_trun = make_full_box(b"trun", 0, trun_flags, &trun_body);
        let mut real_traf_payload = tfhd;
        real_traf_payload.extend_from_slice(&tfdt);
        real_traf_payload.extend_from_slice(&real_trun);
        let real_traf = make_box(b"traf", &real_traf_payload);
        let mut real_moof_payload = mfhd;
        real_moof_payload.extend_from_slice(&real_traf);
        let moof = make_box(b"moof", &real_moof_payload);

        let mut out = Vec::new();
        out.extend_from_slice(&ftyp);
        out.extend_from_slice(&moov);
        out.extend_from_slice(&moof);
        out.extend_from_slice(&mdat);
        out
    }

    #[test]
    fn test_pure_rust_webm_vp9_and_aac_fmp4_remux_to_mkv() {
        let webm_vp9 = build_synthetic_webm_vp9(
            2560,
            1440,
            &[
                (b"\x82\x49\x83\x42\x00\x9F\x05\x9F", 0, true),
                (b"\x86\x01\x02\x03\x04\x05", 33, false),
                (b"\x86\x06\x07\x08\x09\x0A", 66, false),
            ],
        );
        let aud_fmp4 = build_synthetic_fmp4(
            2,
            48_000,
            false,
            &[
                (b"\x21\x10\x04\x60\x8C\x1C", 1024, true),
                (b"\x21\x10\x04\x60\x8C\x1D", 1024, true),
            ],
        );

        let parsed_v = parse_media_samples(&webm_vp9).expect("WebM VP9 parse failed");
        let parsed_a = parse_media_samples(&aud_fmp4).expect("Audio fMP4 parse failed");

        assert_eq!(parsed_v.track_type, TrackType::Video);
        assert_eq!(parsed_v.codec, TrackCodec::Vp9);
        assert_eq!(parsed_v.width, 2560);
        assert_eq!(parsed_v.height, 1440);
        assert_eq!(parsed_v.samples.len(), 3);
        assert!(parsed_v.samples[0].is_sync);
        assert!(!parsed_v.samples[1].is_sync);

        assert_eq!(parsed_a.codec, TrackCodec::Aac);
        assert!(parsed_a.codec_private.as_ref().map(|v| v.len() >= 2).unwrap_or(false));

        // Legacy broken MP4 writer MUST explicitly refuse VP9 instead of emitting unplayable moov boxes
        let bad_mp4_path = std::env::temp_dir().join(format!("octoheft_refuse_{}.mp4", Uuid::new_v4()));
        assert!(write_muxed_mp4(&parsed_v, Some(&parsed_a), &bad_mp4_path).is_err());
        assert!(!bad_mp4_path.exists());

        // Adaptive router selects Matroska (.mkv) for VP9 + AAC mixed codecs
        let requested_dest = std::env::temp_dir().join(format!("octoheft_vp9_mux_{}.mp4", Uuid::new_v4()));
        let (final_path, len) = write_muxed_container(&parsed_v, Some(&parsed_a), &requested_dest)
            .expect("VP9+AAC Matroska write failed");
        assert_eq!(final_path.extension().and_then(|e| e.to_str()), Some("mkv"));
        assert!(len > 0);
        verify_ebml_container_file(&final_path, "matroska", 3, 2)
            .expect("Muxed VP9+AAC MKV failed EBML structural verification");

        let _ = std::fs::remove_file(&final_path);
    }

    #[test]
    fn test_pure_rust_webm_vp9_and_opus_4k_mux_to_webm() {
        let webm_vp9 = build_synthetic_webm_vp9(
            3840,
            2160,
            &[
                (b"\x82\x49\x83\x42\x00\xEF\x08\x6F", 0, true),
                (b"\x86\x01\x02\x03\x04\x05", 16, false),
                (b"\x86\x06\x07\x08\x09\x0A", 33, false),
            ],
        );
        let opus_head = synthesize_default_opus_head(2, 48_000);
        let webm_opus = build_synthetic_webm_track(
            "webm",
            "A_OPUS",
            false,
            0,
            0,
            48_000.0,
            2,
            Some(&opus_head),
            &[
                (b"\xFC\x01\x02\x03\x04", 0, true),
                (b"\xFC\x05\x06\x07\x08", 20, true),
                (b"\xFC\x09\x0A\x0B\x0C", 40, true),
            ],
        );

        let parsed_v = parse_media_samples(&webm_vp9).expect("VP9 parse failed");
        let parsed_a = parse_media_samples(&webm_opus).expect("Opus WebM parse failed");
        assert_eq!(parsed_v.codec, TrackCodec::Vp9);
        assert_eq!(parsed_a.codec, TrackCodec::Opus);
        assert_eq!(parsed_a.seek_pre_roll_ns, 80_000_000);

        let dest = std::env::temp_dir().join(format!("octoheft_4k_opus_{}.webm", Uuid::new_v4()));
        let (out_path, out_len) =
            write_muxed_container(&parsed_v, Some(&parsed_a), &dest).expect("VP9+Opus mux failed");
        assert_eq!(out_path.extension().and_then(|e| e.to_str()), Some("webm"));
        verify_ebml_container_file(&out_path, "webm", 3, 3)
            .expect("Muxed 4K VP9+Opus WebM failed verification");
        assert!(out_len > 0);

        // Verify SeekPreRoll, CodecDelay, and OpusHead survived roundtrip
        let raw = std::fs::read(&out_path).unwrap();
        assert!(raw.windows(8).any(|w| w == b"OpusHead"));
        let _ = std::fs::remove_file(&out_path);
    }

    #[test]
    fn test_pure_rust_isobmff_av1_and_webm_opus_4k_mux_to_webm() {
        let av1_fmp4 = build_synthetic_fmp4_av1(
            1,
            24_000,
            3840,
            2160,
            &[
                (b"\x12\x00\x0A\x01\x20\x30", 1000, true),
                (b"\x12\x00\x0A\x02\x20\x31", 1000, false),
            ],
        );
        let opus_head = synthesize_default_opus_head(2, 48_000);
        let webm_opus = build_synthetic_webm_track(
            "webm",
            "A_OPUS",
            false,
            0,
            0,
            48_000.0,
            2,
            Some(&opus_head),
            &[
                (b"\xFC\x11\x22\x33", 0, true),
                (b"\xFC\x44\x55\x66", 20, true),
            ],
        );

        let parsed_v = parse_media_samples(&av1_fmp4).expect("AV1 fMP4 parse failed");
        let parsed_a = parse_media_samples(&webm_opus).expect("Opus WebM parse failed");
        assert_eq!(parsed_v.codec, TrackCodec::Av1);
        assert_eq!(parsed_v.codec_private.as_deref(), Some(&[0x81, 0x0C, 0x0C, 0x00][..]));
        assert_eq!(parsed_a.codec, TrackCodec::Opus);

        let dest = std::env::temp_dir().join(format!("octoheft_av1_opus_{}.mp4", Uuid::new_v4()));
        let (out_path, _) = write_muxed_container(&parsed_v, Some(&parsed_a), &dest)
            .expect("AV1+Opus mux failed");
        assert_eq!(out_path.extension().and_then(|e| e.to_str()), Some("webm"));
        verify_ebml_container_file(&out_path, "webm", 2, 2).unwrap();

        let _ = std::fs::remove_file(&out_path);
    }

    #[test]
    fn test_mkv_cluster_boundary_split_over_40_seconds() {
        let webm_vp9 = build_synthetic_webm_track(
            "webm",
            "V_VP9",
            true,
            1920,
            1080,
            0.0,
            0,
            None,
            &[
                (b"\x82\x49\x83\x42", 0, true),
                (b"\x82\x49\x83\x42", 42_000, true),
                (b"\x86\x01\x02\x03", 42_033, false),
            ],
        );
        let parsed_v = parse_media_samples(&webm_vp9).expect("Parse >40s WebM");
        assert_eq!(parsed_v.samples.len(), 3);
        let dest = std::env::temp_dir().join(format!("octoheft_cluster_split_{}.webm", Uuid::new_v4()));
        let (out_path, _) = write_muxed_container(&parsed_v, None, &dest).unwrap();
        verify_ebml_container_file(&out_path, "webm", 3, 0).unwrap();

        // Confirm re-parsing preserves exact 42_033ms timestamp without i16 overflow
        let raw = std::fs::read(&out_path).unwrap();
        let reparsed = parse_webm_samples_bytes(Bytes::from(raw)).unwrap();
        assert_eq!(reparsed.samples.len(), 3);
        assert_eq!(reparsed.samples[1].start_time, 42_000);
        assert_eq!(reparsed.samples[2].start_time, 42_033);

        let _ = std::fs::remove_file(&out_path);
    }

    #[test]
    fn test_truncated_containers_rejected_by_demuxers() {
        let valid_fmp4 = build_synthetic_fmp4(
            1,
            30_000,
            true,
            &[
                (b"\x00\x00\x00\x10\x65\x88\x84\x21\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0A\x0B\x0C", 1000, true),
            ],
        );
        let truncated_fmp4 = &valid_fmp4[..valid_fmp4.len() - 10];
        assert!(
            parse_isobmff_samples(truncated_fmp4).is_err(),
            "Truncated ISOBMFF mdat MUST fail instead of silently dropping samples"
        );

        let valid_webm = build_synthetic_webm_vp9(
            1280,
            720,
            &[(b"\x82\x49\x83\x42\x00\x9F\x05\x9F\x11\x22\x33\x44", 0, true)],
        );
        let truncated_webm = &valid_webm[..valid_webm.len() - 6];
        assert!(
            parse_webm_vp9_samples(truncated_webm).is_err(),
            "Truncated WebM Cluster MUST fail instead of returning partial frames"
        );
    }

    #[tokio::test]
    async fn test_parallel_ranged_webm_vp9_and_aac_split_stream_download() {
        // Large VP9 frame (~600 KB) to exercise multi-connection HTTP 206 Partial Content workers + .mkv routing
        let big_keyframe = vec![0x82u8; 600 * 1024];
        let webm_vp9 = Arc::new(build_synthetic_webm_vp9(
            3840,
            2160,
            &[(&big_keyframe, 0, true)],
        ));
        let aud_fmp4 = Arc::new(build_synthetic_fmp4(
            1,
            48_000,
            false,
            &[(b"\x21\x10\x04\x60\x8C\x1C", 1024, true)],
        ));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let range_hits = Arc::new(AtomicU64::new(0));

        let v_clone = Arc::clone(&webm_vp9);
        let a_clone = Arc::clone(&aud_fmp4);
        let rh_clone = Arc::clone(&range_hits);
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let v = Arc::clone(&v_clone);
                let a = Arc::clone(&a_clone);
                let rh = Arc::clone(&rh_clone);
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let path = req
                        .lines()
                        .next()
                        .and_then(|l| l.split_whitespace().nth(1))
                        .unwrap_or("/");
                    let body = if path.contains("audio") { &a[..] } else { &v[..] };
                    let total = body.len();

                    let range_line = req
                        .lines()
                        .find(|l| l.to_ascii_lowercase().starts_with("range:"));

                    if let Some(rl) = range_line {
                        rh.fetch_add(1, Ordering::Relaxed);
                        let spec = rl.split(':').nth(1).unwrap_or("").trim().trim_start_matches("bytes=");
                        let mut parts = spec.split('-');
                        let s = parts.next().and_then(|x| x.parse::<usize>().ok()).unwrap_or(0);
                        let e = parts
                            .next()
                            .and_then(|x| x.parse::<usize>().ok())
                            .unwrap_or(total - 1)
                            .min(total - 1);
                        let slice = &body[s..=e];
                        let hdr = format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {}-{}/{}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                            s,
                            e,
                            total,
                            slice.len()
                        );
                        let _ = sock.write_all(hdr.as_bytes()).await;
                        let _ = sock.write_all(slice).await;
                    } else {
                        let hdr = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                            total
                        );
                        let _ = sock.write_all(hdr.as_bytes()).await;
                        let _ = sock.write_all(body).await;
                    }
                    let _ = sock.shutdown().await;
                });
            }
        });

        let video_url = format!(
            "http://127.0.0.1:{}/videoplayback?mime=video/webm&clen={}",
            addr.port(),
            webm_vp9.len()
        );
        let audio_url = format!(
            "http://127.0.0.1:{}/videoplayback?mime=audio/mp4&clen={}",
            addr.port(),
            aud_fmp4.len()
        );
        let dest = std::env::temp_dir().join(format!("octoheft_4k_ranged_{}.mp4", Uuid::new_v4()));

        let seen_segments = Arc::new(Mutex::new(0usize));
        let ss_clone = Arc::clone(&seen_segments);

        let client = reqwest::Client::new();
        let (final_path, final_len) = MediaMuxer::download_and_remux_split_streams(
            &client,
            &video_url,
            Some(&audio_url),
            None,
            &dest,
            move |prog| {
                let mut g = ss_clone.lock();
                if prog.active_segments.len() > *g {
                    *g = prog.active_segments.len();
                }
            },
        )
        .await
        .expect("Ranged 4K VP9+AAC split download+mux failed");

        assert!(final_len > 500_000);
        assert_eq!(final_path.extension().and_then(|e| e.to_str()), Some("mkv"));
        assert!(
            range_hits.load(Ordering::Relaxed) >= 4,
            "Expected multiple parallel HTTP 206 Range requests"
        );
        assert!(
            *seen_segments.lock() >= 4,
            "Expected multi-connection segment progress reporting"
        );
        verify_ebml_container_file(&final_path, "matroska", 1, 1)
            .expect("Muxed 4K VP9+AAC MKV failed verification");

        let _ = std::fs::remove_file(&final_path);
    }

    #[tokio::test]
    async fn test_range_midstream_drop_resumes_to_eof() {
        let frame_a = vec![0x82u8; 300 * 1024];
        let frame_b = vec![0x86u8; 350 * 1024];
        let webm_vp9 = Arc::new(build_synthetic_webm_vp9(
            3840,
            2160,
            &[(&frame_a, 0, true), (&frame_b, 33, false)],
        ));
        let opus_head = synthesize_default_opus_head(2, 48_000);
        let webm_opus = Arc::new(build_synthetic_webm_track(
            "webm",
            "A_OPUS",
            false,
            0,
            0,
            48_000.0,
            2,
            Some(&opus_head),
            &[(b"\xFC\x01\x02", 0, true)],
        ));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let req_count = Arc::new(AtomicU64::new(0));

        let v_clone = Arc::clone(&webm_vp9);
        let a_clone = Arc::clone(&webm_opus);
        let rc_clone = Arc::clone(&req_count);
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let v = Arc::clone(&v_clone);
                let a = Arc::clone(&a_clone);
                let rc = Arc::clone(&rc_clone);
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let req_lc = req.to_ascii_lowercase();
                    let authorized = req_lc.contains("user-agent: mozilla/5.0 resume")
                        && req_lc.contains("referer: https://www.youtube.com/")
                        && req_lc.contains("origin: https://www.youtube.com")
                        && req_lc.contains("cookie: sid=res_ok");
                    if !authorized {
                        let _ = sock
                            .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
                            .await;
                        let _ = sock.shutdown().await;
                        return;
                    }

                    let hit = rc.fetch_add(1, Ordering::Relaxed);
                    // Inject a transient 503 on one request to test recovery + header preservation
                    if hit == 3 {
                        let _ = sock
                            .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n")
                            .await;
                        let _ = sock.shutdown().await;
                        return;
                    }

                    let path = req
                        .lines()
                        .next()
                        .and_then(|l| l.split_whitespace().nth(1))
                        .unwrap_or("/");
                    let body = if path.contains("audio") { &a[..] } else { &v[..] };
                    let total = body.len();

                    if let Some(rl) = req_lc.lines().find(|l| l.starts_with("range:")) {
                        let spec = rl.split(':').nth(1).unwrap_or("").trim().trim_start_matches("bytes=");
                        let mut parts = spec.split('-');
                        let s = parts.next().and_then(|x| x.parse::<usize>().ok()).unwrap_or(0);
                        let e = parts
                            .next()
                            .and_then(|x| x.parse::<usize>().ok())
                            .unwrap_or(total - 1)
                            .min(total - 1);
                        let slice = &body[s..=e];
                        // Intentionally truncate large video Range responses mid-stream!
                        let hdr = format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {}-{}/{}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                            s, e, total, slice.len()
                        );
                        let _ = sock.write_all(hdr.as_bytes()).await;
                        let cut = if !path.contains("audio") && slice.len() > 24_000 {
                            slice.len() / 2
                        } else {
                            slice.len()
                        };
                        let _ = sock.write_all(&slice[..cut]).await;
                    }
                    let _ = sock.shutdown().await;
                });
            }
        });

        let mut hdrs = HashMap::new();
        hdrs.insert("User-Agent".to_string(), "Mozilla/5.0 resume".to_string());
        hdrs.insert("Referer".to_string(), "https://www.youtube.com/".to_string());
        hdrs.insert("Origin".to_string(), "https://www.youtube.com".to_string());
        hdrs.insert("Cookie".to_string(), "SID=res_ok".to_string());

        let video_url = format!("http://127.0.0.1:{}/videoplayback?mime=video/webm", addr.port());
        let audio_url = format!("http://127.0.0.1:{}/videoplayback?mime=audio/webm", addr.port());
        let dest = std::env::temp_dir().join(format!("octoheft_drop_resume_{}.webm", Uuid::new_v4()));
        let client = reqwest::Client::new();

        let (final_path, final_len) = MediaMuxer::download_and_remux_split_streams(
            &client,
            &video_url,
            Some(&audio_url),
            Some(&hdrs),
            &dest,
            |_| {},
        )
        .await
        .expect("Mid-stream drops MUST recover via byte-exact Range resume");

        assert!(final_len > 640 * 1024);
        assert_eq!(final_path.extension().and_then(|e| e.to_str()), Some("webm"));
        verify_ebml_container_file(&final_path, "webm", 2, 1).unwrap();
        let _ = std::fs::remove_file(&final_path);
    }

    #[tokio::test]
    async fn test_full_stream_truncation_recovers_via_range_resume() {
        let vid_fmp4 = Arc::new(build_synthetic_fmp4(
            1,
            30_000,
            true,
            &[(b"\x00\x00\x00\x04\x65\x88\x84\x21", 1000, true)],
        ));
        let aud_fmp4 = Arc::new(build_synthetic_fmp4(
            1,
            48_000,
            false,
            &[(b"\x21\x10\x04\x60\x8C\x1C", 1024, true)],
        ));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let v_clone = Arc::clone(&vid_fmp4);
        let a_clone = Arc::clone(&aud_fmp4);

        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let v = Arc::clone(&v_clone);
                let a = Arc::clone(&a_clone);
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let req_lc = req.to_ascii_lowercase();
                    let path = req.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("/");
                    let body = if path.contains("audio") { &a[..] } else { &v[..] };
                    let total = body.len();

                    if let Some(rl) = req_lc.lines().find(|l| l.starts_with("range:")) {
                        let spec = rl.split(':').nth(1).unwrap_or("").trim().trim_start_matches("bytes=");
                        let mut parts = spec.split('-');
                        let s = parts.next().and_then(|x| x.parse::<usize>().ok()).unwrap_or(0);
                        let e = parts.next().and_then(|x| x.parse::<usize>().ok()).unwrap_or(total - 1).min(total - 1);
                        // Force FullStream mode by replying 200 OK on initial 0-0 probe, then 206 for resume offsets!
                        if s == 0 && e == 0 {
                            let hdr = format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                total
                            );
                            let _ = sock.write_all(hdr.as_bytes()).await;
                            // Drop cleanly at 50% EOF!
                            let cut = (total / 2).max(16);
                            let _ = sock.write_all(&body[..cut]).await;
                        } else {
                            let slice = &body[s..=e];
                            let hdr = format!(
                                "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {}-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                s, e, total, slice.len()
                            );
                            let _ = sock.write_all(hdr.as_bytes()).await;
                            let _ = sock.write_all(slice).await;
                        }
                    }
                    let _ = sock.shutdown().await;
                });
            }
        });

        let video_url = format!("http://127.0.0.1:{}/videoplayback?mime=video/mp4", addr.port());
        let audio_url = format!("http://127.0.0.1:{}/videoplayback?mime=audio/mp4", addr.port());
        let dest = std::env::temp_dir().join(format!("octoheft_200_eof_{}.mp4", Uuid::new_v4()));
        let client = reqwest::Client::new();

        let (final_path, final_len) = MediaMuxer::download_and_remux_split_streams(
            &client,
            &video_url,
            Some(&audio_url),
            None,
            &dest,
            |_| {},
        )
        .await
        .expect("Premature 200 OK EOF must recover to 100% via Range resume");

        assert!(final_len > 0);
        let reader = Mp4Reader::read_header(File::open(&final_path).unwrap(), final_len).unwrap();
        assert_eq!(reader.tracks().len(), 2);
        let _ = std::fs::remove_file(&final_path);
    }

    #[tokio::test]
    async fn test_unrecoverable_short_stream_fails_without_corrupt_file() {
        let vid_fmp4 = Arc::new(build_synthetic_fmp4(
            1,
            30_000,
            true,
            &[(b"\x00\x00\x00\x04\x65\x88\x84\x21", 1000, true)],
        ));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let v_clone = Arc::clone(&vid_fmp4);

        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let v = Arc::clone(&v_clone);
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let _ = sock.read(&mut buf).await;
                    // Always claim full Content-Length via 200 OK (no Range support) and terminate at 50% EOF
                    let hdr = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        v.len()
                    );
                    let _ = sock.write_all(hdr.as_bytes()).await;
                    let _ = sock.write_all(&v[..v.len() / 2]).await;
                    let _ = sock.shutdown().await;
                });
            }
        });

        let video_url = format!("http://127.0.0.1:{}/videoplayback?mime=video/mp4", addr.port());
        let dest = std::env::temp_dir().join(format!("octoheft_short_fail_{}.mp4", Uuid::new_v4()));
        let client = reqwest::Client::new();

        let res = MediaMuxer::download_and_remux_split_streams(
            &client,
            &video_url,
            None,
            None,
            &dest,
            |_| {},
        )
        .await;

        assert!(
            res.is_err(),
            "Unrecoverable truncated 200 OK stream MUST return Err"
        );
        assert!(
            !dest.exists(),
            "Corrupt output file must NOT be written on stream truncation"
        );
        assert!(!make_part_output_path(&dest).exists());
    }

    #[tokio::test]
    async fn test_split_streams_enforce_session_headers_or_403() {
        let vid_fmp4 = Arc::new(build_synthetic_fmp4(
            1,
            30_000,
            true,
            &[(b"\x00\x00\x00\x04\x65\x88\x84\x21", 1000, true)],
        ));
        let aud_fmp4 = Arc::new(build_synthetic_fmp4(
            1,
            48_000,
            false,
            &[(b"\x21\x10\x04\x60\x8C\x1C", 1024, true)],
        ));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let v_clone = Arc::clone(&vid_fmp4);
        let a_clone = Arc::clone(&aud_fmp4);

        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let v = Arc::clone(&v_clone);
                let a = Arc::clone(&a_clone);
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let req_lc = req.to_ascii_lowercase();
                    let authorized = req_lc.contains("user-agent: mozilla/5.0 live-chrome")
                        && req_lc.contains("referer: https://www.youtube.com/")
                        && req_lc.contains("origin: https://www.youtube.com")
                        && req_lc.contains("cookie: sid=yt_cookie");

                    if !authorized {
                        let hdr = "HTTP/1.1 403 Forbidden\r\nContent-Length: 9\r\nConnection: close\r\n\r\nFORBIDDEN";
                        let _ = sock.write_all(hdr.as_bytes()).await;
                        let _ = sock.shutdown().await;
                        return;
                    }

                    let path = req
                        .lines()
                        .next()
                        .and_then(|l| l.split_whitespace().nth(1))
                        .unwrap_or("/");
                    let body = if path.contains("audio") { &a[..] } else { &v[..] };
                    let total = body.len();

                    if let Some(rl) = req_lc.lines().find(|l| l.starts_with("range:")) {
                        let spec = rl.split(':').nth(1).unwrap_or("").trim().trim_start_matches("bytes=");
                        let mut parts = spec.split('-');
                        let s = parts.next().and_then(|x| x.parse::<usize>().ok()).unwrap_or(0);
                        let e = parts
                            .next()
                            .and_then(|x| x.parse::<usize>().ok())
                            .unwrap_or(total - 1)
                            .min(total - 1);
                        let slice = &body[s..=e];
                        let hdr = format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {}-{}/{}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                            s, e, total, slice.len()
                        );
                        let _ = sock.write_all(hdr.as_bytes()).await;
                        let _ = sock.write_all(slice).await;
                    } else {
                        let hdr = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                            total
                        );
                        let _ = sock.write_all(hdr.as_bytes()).await;
                        let _ = sock.write_all(body).await;
                    }
                    let _ = sock.shutdown().await;
                });
            }
        });

        let video_url = format!("http://127.0.0.1:{}/videoplayback?mime=video/mp4", port);
        let audio_url = format!("http://127.0.0.1:{}/videoplayback?mime=audio/mp4", port);
        let dest = std::env::temp_dir().join(format!("octoheft_403_test_{}.mp4", Uuid::new_v4()));
        let client = reqwest::Client::new();

        // 1. Missing required headers MUST fail explicitly with HTTP 403 Forbidden
        let fail_res = MediaMuxer::download_and_remux_split_streams(
            &client,
            &video_url,
            Some(&audio_url),
            None,
            &dest,
            |_| {},
        )
        .await;
        assert!(fail_res.is_err(), "Missing session headers must trigger 403 failure");
        assert!(
            fail_res.unwrap_err().contains("403"),
            "Expected HTTP 403 Forbidden error"
        );

        // 2. Full Chrome session headers (UA, Referer, Origin, Cookie) MUST succeed
        let mut hdrs = HashMap::new();
        hdrs.insert("User-Agent".to_string(), "Mozilla/5.0 live-chrome".to_string());
        hdrs.insert("Referer".to_string(), "https://www.youtube.com/".to_string());
        hdrs.insert("Origin".to_string(), "https://www.youtube.com".to_string());
        hdrs.insert("Cookie".to_string(), "SID=yt_cookie".to_string());

        let (final_path, final_len) = MediaMuxer::download_and_remux_split_streams(
            &client,
            &video_url,
            Some(&audio_url),
            Some(&hdrs),
            &dest,
            |_| {},
        )
        .await
        .expect("Expected split streams to download and mux with session headers");

        assert!(final_len > 0);
        let reader = Mp4Reader::read_header(File::open(&final_path).unwrap(), final_len).unwrap();
        assert_eq!(reader.tracks().len(), 2);

        let _ = std::fs::remove_file(&final_path);
    }
}
