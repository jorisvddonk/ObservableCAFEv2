use cafe_bus::client::{chunk_matches_filter, session_matches_filter};
use cafe_bus::registry::SessionRegistry;
use cafe_bus::session::SessionState;
use cafe_types::{Chunk, ContentType, SubscribeFilter};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::collections::HashMap;

// ── Helpers ──

fn make_text_chunk(content: &str) -> Chunk {
    Chunk::new_text(content, "bench")
}

fn make_chunk_with_n_annotations(n: usize) -> Chunk {
    let mut c = Chunk::new_text("content", "bench");
    for i in 0..n {
        c = c.with_annotation(format!("k{}", i), format!("v{}", i));
    }
    c
}

fn register_n_sessions(registry: &mut SessionRegistry, n: usize) -> Vec<String> {
    let mut ids = Vec::with_capacity(n);
    for i in 0..n {
        let sid = format!("session-{}", i);
        registry.insert(SessionState::new(sid.clone(), "bench-agent".into()));
        ids.push(sid);
    }
    ids
}

// ── chunk_matches_filter benchmarks ──

fn bench_filter_empty(c: &mut Criterion) {
    let chunk = make_text_chunk("hello");
    let filter = SubscribeFilter::default();
    c.bench_function("chunk_matches_filter/empty", |b| {
        b.iter(|| chunk_matches_filter(black_box(&chunk), black_box(&filter)))
    });
}

fn bench_filter_by_content_type(c: &mut Criterion) {
    let chunk = make_text_chunk("hello");
    let filter = SubscribeFilter {
        content_types: Some(vec![ContentType::Text]),
        ..Default::default()
    };
    c.bench_function("chunk_matches_filter/by_content_type", |b| {
        b.iter(|| chunk_matches_filter(black_box(&chunk), black_box(&filter)))
    });
}

fn bench_filter_by_content_type_miss(c: &mut Criterion) {
    let chunk = make_text_chunk("hello");
    let filter = SubscribeFilter {
        content_types: Some(vec![ContentType::Binary]),
        ..Default::default()
    };
    c.bench_function("chunk_matches_filter/by_content_type_miss", |b| {
        b.iter(|| chunk_matches_filter(black_box(&chunk), black_box(&filter)))
    });
}

fn bench_filter_by_annotation(c: &mut Criterion) {
    let chunk = make_chunk_with_n_annotations(5);
    let filter = SubscribeFilter {
        annotations: Some(HashMap::from([(
            "k0".into(),
            serde_json::Value::String("v0".into()),
        )])),
        ..Default::default()
    };
    c.bench_function("chunk_matches_filter/1_annotation_hit", |b| {
        b.iter(|| chunk_matches_filter(black_box(&chunk), black_box(&filter)))
    });

    let filter_miss = SubscribeFilter {
        annotations: Some(HashMap::from([(
            "missing".into(),
            serde_json::Value::String("nope".into()),
        )])),
        ..Default::default()
    };
    c.bench_function("chunk_matches_filter/1_annotation_miss", |b| {
        b.iter(|| chunk_matches_filter(black_box(&chunk), black_box(&filter_miss)))
    });
}

fn bench_filter_by_10_annotations(c: &mut Criterion) {
    let chunk = make_chunk_with_n_annotations(10);
    let mut ann_map = HashMap::new();
    for i in 0..10 {
        ann_map.insert(
            format!("k{}", i),
            serde_json::Value::String(format!("v{}", i)),
        );
    }
    let filter = SubscribeFilter {
        annotations: Some(ann_map),
        ..Default::default()
    };
    c.bench_function("chunk_matches_filter/10_annotations_hit", |b| {
        b.iter(|| chunk_matches_filter(black_box(&chunk), black_box(&filter)))
    });
}

fn bench_filter_combined(c: &mut Criterion) {
    let chunk = make_chunk_with_n_annotations(5);
    let filter = SubscribeFilter {
        content_types: Some(vec![ContentType::Text]),
        annotations: Some(HashMap::from([(
            "k0".into(),
            serde_json::Value::String("v0".into()),
        )])),
        ..Default::default()
    };
    c.bench_function("chunk_matches_filter/combined_type+annotation_hit", |b| {
        b.iter(|| chunk_matches_filter(black_box(&chunk), black_box(&filter)))
    });
}

// ── session_matches_filter benchmarks ──

fn bench_session_filter_empty(c: &mut Criterion) {
    let session = SessionState::new("s-1".into(), "agent-1".into());
    let filter = SubscribeFilter::default();
    c.bench_function("session_matches_filter/empty", |b| {
        b.iter(|| session_matches_filter(black_box(&session), black_box(&filter)))
    });
}

fn bench_session_filter_by_id(c: &mut Criterion) {
    let session = SessionState::new("s-1".into(), "agent-1".into());
    let filter = SubscribeFilter {
        sessions: Some(vec!["s-1".into()]),
        ..Default::default()
    };
    c.bench_function("session_matches_filter/by_id_hit", |b| {
        b.iter(|| session_matches_filter(black_box(&session), black_box(&filter)))
    });
}

fn bench_session_filter_by_id_miss(c: &mut Criterion) {
    let session = SessionState::new("s-1".into(), "agent-1".into());
    let filter = SubscribeFilter {
        sessions: Some(vec!["other".into()]),
        ..Default::default()
    };
    c.bench_function("session_matches_filter/by_id_miss", |b| {
        b.iter(|| session_matches_filter(black_box(&session), black_box(&filter)))
    });
}

fn bench_session_filter_combined(c: &mut Criterion) {
    let session = SessionState::new("s-1".into(), "agent-1".into());
    let filter = SubscribeFilter {
        sessions: Some(vec!["s-1".into()]),
        agents: Some(vec!["agent-1".into()]),
        ..Default::default()
    };
    c.bench_function("session_matches_filter/by_id+agent_hit", |b| {
        b.iter(|| session_matches_filter(black_box(&session), black_box(&filter)))
    });
}

fn bench_session_filter_large_set(c: &mut Criterion) {
    let session = SessionState::new("target".into(), "target-agent".into());
    let mut session_ids = (0..50).map(|i| format!("session-{}", i)).collect::<Vec<_>>();
    session_ids.push("target".into());
    let filter = SubscribeFilter {
        sessions: Some(session_ids),
        ..Default::default()
    };
    c.bench_function("session_matches_filter/50_ids_hit_last", |b| {
        b.iter(|| session_matches_filter(black_box(&session), black_box(&filter)))
    });
}

// ── SessionState::publish benchmarks ──

fn bench_publish_empty_history(c: &mut Criterion) {
    let mut session = SessionState::new("s-1".into(), "bench".into());
    let chunk = make_text_chunk("hello");
    c.bench_function("SessionState::publish/empty_history", |b| {
        b.iter(|| session.publish(black_box(chunk.clone())))
    });
}

fn bench_publish_100_history(c: &mut Criterion) {
    let mut session = SessionState::new("s-1".into(), "bench".into());
    for i in 0..100 {
        session.publish(make_text_chunk(&format!("history chunk {}", i)));
    }
    let chunk = make_text_chunk("hello");
    c.bench_function("SessionState::publish/100_history", |b| {
        b.iter(|| session.publish(black_box(chunk.clone())))
    });
}

fn bench_publish_1000_history(c: &mut Criterion) {
    let mut session = SessionState::new("s-1".into(), "bench".into());
    for i in 0..1000 {
        session.publish(make_text_chunk(&format!("history chunk {}", i)));
    }
    let chunk = make_text_chunk("hello");
    c.bench_function("SessionState::publish/1000_history", |b| {
        b.iter(|| session.publish(black_box(chunk.clone())))
    });
}

fn bench_publish_transient(c: &mut Criterion) {
    let mut session = SessionState::new("s-1".into(), "bench".into());
    for i in 0..100 {
        session.publish(make_text_chunk(&format!("history chunk {}", i)));
    }
    let mut chunk = make_text_chunk("transient");
    chunk = chunk.as_transient();
    c.bench_function("SessionState::publish/transient_100_history", |b| {
        b.iter(|| session.publish(black_box(chunk.clone())))
    });
}

// ── SessionRegistry benchmarks ──

fn bench_registry_insert(c: &mut Criterion) {
    let mut registry = SessionRegistry::new();
    c.bench_function("SessionRegistry/insert", |b| {
        b.iter(|| {
            let sid = black_box("test-session".to_string());
            if !registry.contains(&sid) {
                registry.insert(SessionState::new(sid.clone(), "bench".into()));
            }
        })
    });
}

fn bench_registry_get_hit(c: &mut Criterion) {
    let mut registry = SessionRegistry::new();
    register_n_sessions(&mut registry, 1);
    c.bench_function("SessionRegistry/get/1_session_hit", |b| {
        b.iter(|| registry.get(black_box("session-0")))
    });
}

fn bench_registry_get_miss(c: &mut Criterion) {
    let mut registry = SessionRegistry::new();
    register_n_sessions(&mut registry, 100);
    c.bench_function("SessionRegistry/get/100_sessions_miss", |b| {
        b.iter(|| registry.get(black_box("nonexistent")))
    });
}

fn bench_registry_get_1000_hit(c: &mut Criterion) {
    let mut registry = SessionRegistry::new();
    register_n_sessions(&mut registry, 1000);
    c.bench_function("SessionRegistry/get/1000_sessions_hit_last", |b| {
        b.iter(|| registry.get(black_box("session-999")))
    });
}

fn bench_registry_remove(c: &mut Criterion) {
    let mut registry = SessionRegistry::new();
    register_n_sessions(&mut registry, 100);
    c.bench_function("SessionRegistry/remove/100_sessions_hit", |b| {
        b.iter(|| {
            let mut r = SessionRegistry::new();
            register_n_sessions(&mut r, 100);
            r.remove(black_box("session-50"));
        })
    });
}

fn bench_registry_list(c: &mut Criterion) {
    let mut registry = SessionRegistry::new();
    register_n_sessions(&mut registry, 100);
    c.bench_function("SessionRegistry/list/100_sessions", |b| {
        b.iter(|| registry.list())
    });
}

criterion_group!(
    benches,
    // chunk_matches_filter
    bench_filter_empty,
    bench_filter_by_content_type,
    bench_filter_by_content_type_miss,
    bench_filter_by_annotation,
    bench_filter_by_10_annotations,
    bench_filter_combined,
    // session_matches_filter
    bench_session_filter_empty,
    bench_session_filter_by_id,
    bench_session_filter_by_id_miss,
    bench_session_filter_combined,
    bench_session_filter_large_set,
    // SessionState::publish
    bench_publish_empty_history,
    bench_publish_100_history,
    bench_publish_1000_history,
    bench_publish_transient,
    // SessionRegistry
    bench_registry_insert,
    bench_registry_get_hit,
    bench_registry_get_miss,
    bench_registry_get_1000_hit,
    bench_registry_remove,
    bench_registry_list,
);
criterion_main!(benches);
