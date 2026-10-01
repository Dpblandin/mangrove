//! Synthetic graph probe; excludes Git, commit metadata, and GPUI rendering.
//! Run with: cargo run --release --example graph_benchmark -- 5000 128 5
use mangrove_core::model;
use std::{
    error::Error,
    hash::{Hash, Hasher},
    hint::black_box,
    mem::size_of,
    time::Instant,
};

#[path = "../src/graph.rs"]
mod graph;

fn oid(namespace: usize, index: usize) -> String {
    format!("{namespace:08x}{index:032x}")
}

fn argument(index: usize, default: usize) -> Result<usize, Box<dyn Error>> {
    let value = std::env::args()
        .nth(index)
        .map(|value| value.parse::<usize>())
        .transpose()?
        .unwrap_or(default);
    if value == 0 {
        return Err("lanes, rows, and samples must be positive".into());
    }
    Ok(value)
}

fn median(samples: &[f64]) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        (sorted[middle - 1] + sorted[middle]) / 2.
    } else {
        sorted[middle]
    }
}

fn geometry_digest(rows: &[model::GraphRow]) -> String {
    let mut digest = std::collections::hash_map::DefaultHasher::new();
    for row in rows {
        row.lane.hash(&mut digest);
        row.color.hash(&mut digest);
        row.width.hash(&mut digest);
        for edge in &row.edges {
            edge.from.hash(&mut digest);
            edge.to.hash(&mut digest);
            edge.color.hash(&mut digest);
            match edge.kind {
                model::GraphEdgeKind::Through => "through",
                model::GraphEdgeKind::Incoming => "incoming",
                model::GraphEdgeKind::Parent => "parent",
            }
            .hash(&mut digest);
        }
    }
    format!("{:016x}", digest.finish())
}

fn main() -> Result<(), Box<dyn Error>> {
    let lanes = argument(1, 5000)?;
    let row_count = argument(2, 128)?;
    let sample_count = argument(3, 5)?;
    if std::env::args().len() > 4 {
        return Err("usage: graph_benchmark [lanes] [rows] [samples]".into());
    }
    let mut generation = Vec::with_capacity(sample_count);
    let mut destruction = Vec::with_capacity(sample_count);
    let mut bytes = 0;
    let mut edges = 0;
    let mut digest = String::new();
    for _ in 0..sample_count {
        let mut layout = graph::Layout::default();
        for index in 0..lanes {
            black_box(layout.push(&oid(1, index), &[oid(2, index)]));
        }
        let started = Instant::now();
        let rows: Vec<_> = (0..row_count)
            .map(|index| {
                let sha = if index == 0 {
                    oid(2, 0)
                } else {
                    oid(3, index - 1)
                };
                layout.push(&sha, &[oid(3, index)])
            })
            .collect();
        generation.push(started.elapsed().as_secs_f64() * 1000.);
        edges = rows.iter().map(|row| row.edges.len()).sum::<usize>();
        // GraphEdgeKind has no heap payload. Count owned vector capacities,
        // rather than lengths; this is an allocation estimate, not an RSS.
        bytes = rows.capacity() * size_of::<model::GraphRow>()
            + rows
                .iter()
                .map(|row| row.edges.capacity() * size_of::<model::GraphEdge>())
                .sum::<usize>();
        if digest.is_empty() {
            digest = geometry_digest(&rows);
        }
        black_box(&rows);
        let started = Instant::now();
        drop(rows);
        destruction.push(started.elapsed().as_secs_f64() * 1000.);
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "operation": "graph_layout",
            "lanes": lanes,
            "rows": row_count,
            "samples": sample_count,
            "edgeSizeBytes": size_of::<model::GraphEdge>(),
            "edges": edges,
            "graphAllocationBytes": bytes,
            "geometryDigest": digest,
            "generationMedianMs": median(&generation),
            "generationSamplesMs": generation,
            "destructionMedianMs": median(&destruction),
            "destructionSamplesMs": destruction,
        }))?
    );
    Ok(())
}
