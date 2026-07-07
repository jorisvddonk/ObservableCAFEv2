use criterion::{black_box, criterion_group, criterion_main, Criterion};
use cafe_types::{Chunk, ClientMessage, ServerMessage};
use serde::de::DeserializeOwned;
use serde::Serialize;

/// A serialization format adapter. Implement this to add a new format.
///
/// To add a new format:
///   1. Define a struct (e.g. `struct SimdJsonFormat`)
///   2. Implement `Format` for it
///   3. Add `format_benches!(c, SimdJsonFormat)` in a wrapper function
///   4. Add that wrapper to `criterion_group!`
pub trait Format {
    const NAME: &'static str;
    fn serialize<T: Serialize>(value: &T) -> String;
    fn deserialize<T: DeserializeOwned>(s: &str) -> T;
}

pub struct JsonFormat;
impl Format for JsonFormat {
    const NAME: &'static str = "json";
    fn serialize<T: Serialize>(value: &T) -> String {
        serde_json::to_string(value).expect("serialize failed")
    }
    fn deserialize<T: DeserializeOwned>(s: &str) -> T {
        serde_json::from_str(s).expect("deserialize failed")
    }
}

// ── Benchmark data helpers ──

fn make_small_text_chunk() -> Chunk {
    let mut c = Chunk::new_text("Hello, world!", "bench");
    c.id = "00000000-0000-0000-0000-000000000001".into();
    c
}

fn make_large_text_chunk() -> Chunk {
    let content = "A".repeat(10_000);
    let mut c = Chunk::new_text(content, "bench");
    c.id = "00000000-0000-0000-0000-000000000001".into();
    c = c.with_annotation("key1", "value1");
    c = c.with_annotation("key2", "value2");
    c = c.with_annotation("key3", "value3");
    c = c.with_annotation("key4", "value4");
    c = c.with_annotation("key5", "value5");
    c
}

fn make_binary_chunk() -> Chunk {
    let data = vec![0xABu8; 1024];
    let mut c = Chunk::new_binary(data, "image/png", "bench");
    c.id = "00000000-0000-0000-0000-000000000001".into();
    c
}

fn make_annotated_chunk() -> Chunk {
    let mut c = Chunk::new_text("content", "bench");
    c.id = "00000000-0000-0000-0000-000000000001".into();
    for i in 0..10 {
        c = c.with_annotation(format!("key{}", i), format!("value{}", i));
    }
    c
}

fn make_publish_msg() -> ClientMessage {
    ClientMessage::Publish {
        session_id: "test-session-id".into(),
        chunk: make_small_text_chunk(),
    }
}

fn make_server_chunk_msg() -> ServerMessage {
    ServerMessage::Chunk {
        session_id: "test-session-id".into(),
        chunk: make_small_text_chunk(),
    }
}

// ── Generic benchmark functions ──

fn bench_chunk_serialize<F: Format>(c: &mut Criterion) {
    let small = make_small_text_chunk();
    let large = make_large_text_chunk();
    let bin = make_binary_chunk();
    let ann = make_annotated_chunk();

    let mut group = c.benchmark_group(format!("Chunk/serialize/{}", F::NAME));
    group.bench_function("small_text", |b| b.iter(|| F::serialize(black_box(&small))));
    group.bench_function("large_text_10k", |b| b.iter(|| F::serialize(black_box(&large))));
    group.bench_function("binary_1k", |b| b.iter(|| F::serialize(black_box(&bin))));
    group.bench_function("annotated_10", |b| b.iter(|| F::serialize(black_box(&ann))));
    group.finish();
}

fn bench_chunk_deserialize<F: Format>(c: &mut Criterion) {
    let small_enc = F::serialize(&make_small_text_chunk());
    let large_enc = F::serialize(&make_large_text_chunk());
    let bin_enc = F::serialize(&make_binary_chunk());
    let ann_enc = F::serialize(&make_annotated_chunk());

    let mut group = c.benchmark_group(format!("Chunk/deserialize/{}", F::NAME));
    group.bench_function("small_text", |b| b.iter(|| F::deserialize::<Chunk>(black_box(&small_enc))));
    group.bench_function("large_text_10k", |b| b.iter(|| F::deserialize::<Chunk>(black_box(&large_enc))));
    group.bench_function("binary_1k", |b| b.iter(|| F::deserialize::<Chunk>(black_box(&bin_enc))));
    group.bench_function("annotated_10", |b| b.iter(|| F::deserialize::<Chunk>(black_box(&ann_enc))));
    group.finish();
}

fn bench_chunk_roundtrip<F: Format>(c: &mut Criterion) {
    let small = make_small_text_chunk();
    c.bench_function(&format!("Chunk/roundtrip/{}/small_text", F::NAME), |b| {
        b.iter(|| {
            let enc = F::serialize(black_box(&small));
            let _: Chunk = F::deserialize(&enc);
        })
    });
}

fn bench_envelope_serialize<F: Format>(c: &mut Criterion) {
    let publish = make_publish_msg();
    let server_chunk = make_server_chunk_msg();
    let connected = ServerMessage::Connected {
        connection_id: "c-42".into(),
    };
    let error = ServerMessage::Error {
        session_id: Some("s-1".into()),
        message: "something went wrong".into(),
        code: "ERR_001".into(),
    };
    let session_created = ServerMessage::SessionCreated {
        session_id: "s-1".into(),
        agent_id: "default".into(),
    };
    let history_complete = ServerMessage::HistoryComplete {
        session_id: "s-1".into(),
        count: 42,
    };

    let mut group = c.benchmark_group(format!("ClientMessage/serialize/{}", F::NAME));
    group.bench_function("publish_text", |b| b.iter(|| F::serialize(black_box(&publish))));
    group.finish();

    let mut group = c.benchmark_group(format!("ServerMessage/serialize/{}", F::NAME));
    group.bench_function("chunk_text", |b| b.iter(|| F::serialize(black_box(&server_chunk))));
    group.bench_function("connected", |b| b.iter(|| F::serialize(black_box(&connected))));
    group.bench_function("error", |b| b.iter(|| F::serialize(black_box(&error))));
    group.bench_function("session_created", |b| b.iter(|| F::serialize(black_box(&session_created))));
    group.bench_function("history_complete", |b| b.iter(|| F::serialize(black_box(&history_complete))));
    group.finish();
}

fn bench_envelope_deserialize<F: Format>(c: &mut Criterion) {
    let publish_enc = F::serialize(&make_publish_msg());
    let server_chunk_enc = F::serialize(&make_server_chunk_msg());
    let connected_enc = F::serialize(&ServerMessage::Connected {
        connection_id: "c-42".into(),
    });
    let error_enc = F::serialize(&ServerMessage::Error {
        session_id: Some("s-1".into()),
        message: "something went wrong".into(),
        code: "ERR_001".into(),
    });

    let mut group = c.benchmark_group(format!("ClientMessage/deserialize/{}", F::NAME));
    group.bench_function("publish_text", |b| {
        b.iter(|| F::deserialize::<ClientMessage>(black_box(&publish_enc)))
    });
    group.finish();

    let mut group = c.benchmark_group(format!("ServerMessage/deserialize/{}", F::NAME));
    group.bench_function("chunk_text", |b| {
        b.iter(|| F::deserialize::<ServerMessage>(black_box(&server_chunk_enc)))
    });
    group.bench_function("connected", |b| {
        b.iter(|| F::deserialize::<ServerMessage>(black_box(&connected_enc)))
    });
    group.bench_function("error", |b| {
        b.iter(|| F::deserialize::<ServerMessage>(black_box(&error_enc)))
    });
    group.finish();
}

fn bench_client_message_variants<F: Format>(c: &mut Criterion) {
    let subscribe = ClientMessage::Subscribe {
        session_id: "s-1".into(),
    };
    let create = ClientMessage::CreateSession {
        session_id: "s-1".into(),
        agent_id: "default".into(),
        config: Default::default(),
    };
    let delete = ClientMessage::DeleteSession {
        session_id: "s-1".into(),
    };

    let mut group = c.benchmark_group(format!("ClientMessage/variants/{}", F::NAME));
    group.bench_function("subscribe", |b| b.iter(|| F::serialize(black_box(&subscribe))));
    group.bench_function("create_session", |b| b.iter(|| F::serialize(black_box(&create))));
    group.bench_function("delete_session", |b| b.iter(|| F::serialize(black_box(&delete))));
    group.bench_function("ping", |b| b.iter(|| F::serialize(black_box(&ClientMessage::Ping))));
    group.bench_function("list", |b| {
        b.iter(|| F::serialize(black_box(&ClientMessage::ListSessions)))
    });
    group.finish();
}

/// Register all benchmark categories for a given format.
macro_rules! format_benches {
    ($c:ident, $format:ty) => {{
        bench_chunk_serialize::<$format>($c);
        bench_chunk_deserialize::<$format>($c);
        bench_chunk_roundtrip::<$format>($c);
        bench_envelope_serialize::<$format>($c);
        bench_envelope_deserialize::<$format>($c);
        bench_client_message_variants::<$format>($c);
    }};
}

fn bench_all_json(c: &mut Criterion) {
    format_benches!(c, JsonFormat);
}

criterion_group!(benches, bench_all_json);
criterion_main!(benches);
