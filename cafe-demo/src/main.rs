use anyhow::Result;
use cafe_sdk::bus::BusClient;
use cafe_sdk::Chunk;
use std::time::Duration;

const SOCKET_PATH: &str = "/tmp/cafe-bus.sock";
const PRODUCER: &str = "com.nominal.cafe-demo";
const SESSION_ID: &str = "demo";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    tracing::info!("cafe-demo: starting");

    cafe_sdk::bus::wait_for_bus(SOCKET_PATH, Duration::from_millis(500), 60).await?;

    let bus = BusClient::new(SOCKET_PATH);

    bus.create_session(
        SESSION_ID,
        SESSION_ID,
        cafe_sdk::SessionConfig::default(),
    )
    .await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    tracing::info!("cafe-demo: publishing text chunk");
    let text_chunk = Chunk::new_text(
        "Hello! This is a demo session demonstrating all chunk types.",
        PRODUCER,
    )
    .with_annotation(cafe_sdk::keys::CHAT_ROLE, "assistant");
    bus.publish(SESSION_ID, text_chunk).await?;

    tracing::info!("cafe-demo: publishing audio chunk");
    let wav_data = generate_demo_wav();
    let audio_chunk = Chunk::new_binary(wav_data, "audio/wav", PRODUCER)
        .with_annotation(cafe_sdk::keys::CHAT_ROLE, "assistant");
    bus.publish(SESSION_ID, audio_chunk).await?;

    tracing::info!("cafe-demo: publishing image chunk");
    let png_data = generate_demo_png();
    let image_chunk = Chunk::new_binary(png_data, "image/png", PRODUCER)
        .with_annotation(cafe_sdk::keys::CHAT_ROLE, "assistant");
    bus.publish(SESSION_ID, image_chunk).await?;

    tracing::info!("cafe-demo: publishing error chunk");
    let error_chunk = Chunk::new_null(PRODUCER)
        .with_annotation(cafe_sdk::keys::ERROR_MESSAGE, "This is a demo error — not a real problem")
        .with_annotation(cafe_sdk::keys::ERROR_CODE, "DEMO_ERROR")
        .with_annotation(cafe_sdk::keys::CHAT_ROLE, "assistant");
    bus.publish(SESSION_ID, error_chunk).await?;

    tracing::info!("cafe-demo: done — published 4 chunks to session '{}'", SESSION_ID);
    Ok(())
}

fn generate_demo_wav() -> Vec<u8> {
    let sample_rate = 8000u32;
    let num_samples = (sample_rate as f64 * 0.5) as u32;
    let data_size = num_samples * 2;
    let file_size = 44 + data_size;

    let mut wav = Vec::with_capacity(file_size as usize);

    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(file_size as u32 - 8).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());

    for i in 0..num_samples {
        let t = i as f64 / sample_rate as f64;
        let sample = (t * 440.0 * 2.0 * std::f64::consts::PI).sin();
        let sample_i16 = (sample * 0.7 * 32767.0) as i16;
        wav.extend_from_slice(&sample_i16.to_le_bytes());
    }

    wav
}

fn generate_demo_png() -> Vec<u8> {
    vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
        0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR chunk
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
        0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
        0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, // IDAT chunk
        0x54, 0x08, 0xD7, 0x63, 0x60, 0xF8, 0xCF, 0x50,
        0x0F, 0x00, 0x06, 0x18, 0x06, 0x00, 0x5A, 0x34,
        0x7D, 0x6B, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
        0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82, // IEND chunk
    ]
}
