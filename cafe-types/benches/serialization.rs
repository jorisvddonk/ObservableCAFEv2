use criterion::{black_box, criterion_group, criterion_main, Criterion};
use cafe_types::{Chunk, ClientMessage, ServerMessage};

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

fn bench_chunk_serialize(c: &mut Criterion) {
    let small = make_small_text_chunk();
    let large = make_large_text_chunk();
    let bin = make_binary_chunk();
    let ann = make_annotated_chunk();

    let mut group = c.benchmark_group("Chunk/serialize");
    group.bench_function("small_text", |b| b.iter(|| serde_json::to_string(black_box(&small))));
    group.bench_function("large_text_10k", |b| b.iter(|| serde_json::to_string(black_box(&large))));
    group.bench_function("binary_1k", |b| b.iter(|| serde_json::to_string(black_box(&bin))));
    group.bench_function("annotated_10", |b| b.iter(|| serde_json::to_string(black_box(&ann))));
    group.finish();
}

fn bench_chunk_deserialize(c: &mut Criterion) {
    let small_json = serde_json::to_string(&make_small_text_chunk()).unwrap();
    let large_json = serde_json::to_string(&make_large_text_chunk()).unwrap();
    let bin_json = serde_json::to_string(&make_binary_chunk()).unwrap();
    let ann_json = serde_json::to_string(&make_annotated_chunk()).unwrap();

    let mut group = c.benchmark_group("Chunk/deserialize");
    group.bench_function("small_text", |b| b.iter(|| serde_json::from_str::<Chunk>(black_box(&small_json))));
    group.bench_function("large_text_10k", |b| b.iter(|| serde_json::from_str::<Chunk>(black_box(&large_json))));
    group.bench_function("binary_1k", |b| b.iter(|| serde_json::from_str::<Chunk>(black_box(&bin_json))));
    group.bench_function("annotated_10", |b| b.iter(|| serde_json::from_str::<Chunk>(black_box(&ann_json))));
    group.finish();
}

fn bench_chunk_roundtrip(c: &mut Criterion) {
    let small = make_small_text_chunk();
    c.bench_function("Chunk/roundtrip/small_text", |b| {
        b.iter(|| {
            let json = serde_json::to_string(black_box(&small)).unwrap();
            let _: Chunk = serde_json::from_str(&json).unwrap();
        })
    });
}

fn bench_envelope_serialize(c: &mut Criterion) {
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

    let mut group = c.benchmark_group("ClientMessage/serialize");
    group.bench_function("publish_text", |b| b.iter(|| serde_json::to_string(black_box(&publish))));
    group.finish();

    let mut group = c.benchmark_group("ServerMessage/serialize");
    group.bench_function("chunk_text", |b| b.iter(|| serde_json::to_string(black_box(&server_chunk))));
    group.bench_function("connected", |b| b.iter(|| serde_json::to_string(black_box(&connected))));
    group.bench_function("error", |b| b.iter(|| serde_json::to_string(black_box(&error))));
    group.bench_function("session_created", |b| b.iter(|| serde_json::to_string(black_box(&session_created))));
    group.bench_function("history_complete", |b| b.iter(|| serde_json::to_string(black_box(&history_complete))));
    group.finish();
}

fn bench_envelope_deserialize(c: &mut Criterion) {
    let publish_json = serde_json::to_string(&make_publish_msg()).unwrap();
    let server_chunk_json = serde_json::to_string(&make_server_chunk_msg()).unwrap();
    let connected_json = serde_json::to_string(&ServerMessage::Connected {
        connection_id: "c-42".into(),
    })
    .unwrap();
    let error_json = serde_json::to_string(&ServerMessage::Error {
        session_id: Some("s-1".into()),
        message: "something went wrong".into(),
        code: "ERR_001".into(),
    })
    .unwrap();

    let mut group = c.benchmark_group("ClientMessage/deserialize");
    group.bench_function("publish_text", |b| {
        b.iter(|| serde_json::from_str::<ClientMessage>(black_box(&publish_json)))
    });
    group.finish();

    let mut group = c.benchmark_group("ServerMessage/deserialize");
    group.bench_function("chunk_text", |b| {
        b.iter(|| serde_json::from_str::<ServerMessage>(black_box(&server_chunk_json)))
    });
    group.bench_function("connected", |b| {
        b.iter(|| serde_json::from_str::<ServerMessage>(black_box(&connected_json)))
    });
    group.bench_function("error", |b| {
        b.iter(|| serde_json::from_str::<ServerMessage>(black_box(&error_json)))
    });
    group.finish();
}

fn bench_client_message_variants(c: &mut Criterion) {
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

    let mut group = c.benchmark_group("ClientMessage/variants");
    group.bench_function("subscribe", |b| b.iter(|| serde_json::to_string(black_box(&subscribe))));
    group.bench_function("create_session", |b| b.iter(|| serde_json::to_string(black_box(&create))));
    group.bench_function("delete_session", |b| b.iter(|| serde_json::to_string(black_box(&delete))));
    group.bench_function("ping", |b| b.iter(|| serde_json::to_string(black_box(&ClientMessage::Ping))));
    group.bench_function("list", |b| {
        b.iter(|| serde_json::to_string(black_box(&ClientMessage::ListSessions)))
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_chunk_serialize,
    bench_chunk_deserialize,
    bench_chunk_roundtrip,
    bench_envelope_serialize,
    bench_envelope_deserialize,
    bench_client_message_variants,
);
criterion_main!(benches);
