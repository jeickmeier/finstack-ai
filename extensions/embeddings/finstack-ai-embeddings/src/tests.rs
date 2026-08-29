use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use crate::embedder::{EmbedError, HashEmbedder, TextEmbedder};
use crate::vector::{EMBEDDING_MAX_DIMENSIONS, EmbeddingVector, truncate_to_bytes};

fn hash_of(vector: &EmbeddingVector) -> u64 {
    let mut hasher = DefaultHasher::new();
    vector.hash(&mut hasher);
    hasher.finish()
}

#[test]
fn vector_rejects_empty_components() {
    assert_eq!(
        EmbeddingVector::try_new(Vec::new()),
        Err(EmbedError::InvalidInput {
            reason: "embedding_components_empty",
        })
    );
}

#[test]
fn vector_rejects_oversized_components() {
    let oversized = vec![1.0_f32; EMBEDDING_MAX_DIMENSIONS + 1];
    assert_eq!(
        EmbeddingVector::try_new(oversized),
        Err(EmbedError::InvalidInput {
            reason: "embedding_dimensions_exceeded",
        })
    );
    assert!(EmbeddingVector::try_new(vec![1.0_f32; EMBEDDING_MAX_DIMENSIONS]).is_ok());
}

#[test]
fn vector_rejects_non_finite_components() {
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(
            EmbeddingVector::try_new(vec![1.0, bad]),
            Err(EmbedError::InvalidInput {
                reason: "embedding_component_not_finite",
            })
        );
    }
}

#[test]
fn vector_rejects_all_zero_components() {
    for zeros in [vec![0.0_f32], vec![0.0_f32, -0.0_f32]] {
        assert_eq!(
            EmbeddingVector::try_new(zeros),
            Err(EmbedError::InvalidInput {
                reason: "embedding_zero_vector",
            })
        );
    }
    // A single nonzero component is enough for a defined norm.
    assert!(EmbeddingVector::try_new(vec![0.0, 0.5, 0.0]).is_ok());
}

#[test]
fn vector_equality_and_hash_agree_on_bit_patterns() {
    let left = EmbeddingVector::try_new(vec![0.5, -1.25, 3.0]).unwrap();
    let right = EmbeddingVector::try_new(vec![0.5, -1.25, 3.0]).unwrap();
    assert_eq!(left, right);
    assert_eq!(hash_of(&left), hash_of(&right));

    let different = EmbeddingVector::try_new(vec![0.5, -1.25, 3.5]).unwrap();
    assert_ne!(left, different);

    // Equality is over exact bit patterns: +0.0 and -0.0 are distinct.
    let positive_zero = EmbeddingVector::try_new(vec![1.0, 0.0]).unwrap();
    let negative_zero = EmbeddingVector::try_new(vec![1.0, -0.0]).unwrap();
    assert_ne!(positive_zero, negative_zero);
}

#[test]
fn unit_normalized_yields_unit_norm_and_is_idempotent() {
    let vector = EmbeddingVector::try_new(vec![3.0, 4.0]).unwrap();
    let normalized = vector.unit_normalized();
    assert_eq!(normalized.as_slice(), &[0.6, 0.8]);

    let norm = normalized.dot(&normalized).unwrap();
    assert!((norm - 1.0).abs() < 1e-6, "norm^2 was {norm}");

    assert_eq!(normalized.unit_normalized(), normalized);

    let awkward = EmbeddingVector::try_new(vec![0.1, -7.3, 2.5, 0.0]).unwrap();
    let once = awkward.unit_normalized();
    let twice = once.unit_normalized();
    assert_eq!(once, twice);
    let norm = once.dot(&once).unwrap();
    assert!((norm - 1.0).abs() < 1e-6, "norm^2 was {norm}");
}

#[test]
fn dot_is_none_on_dimension_mismatch() {
    let two = EmbeddingVector::try_new(vec![1.0, 0.0]).unwrap();
    let three = EmbeddingVector::try_new(vec![1.0, 0.0, 0.0]).unwrap();
    assert_eq!(two.dot(&three), None);
    assert_eq!(three.dot(&two), None);
}

#[test]
fn dot_matches_expected_values() {
    let x_axis = EmbeddingVector::try_new(vec![1.0, 0.0]).unwrap();
    let y_axis = EmbeddingVector::try_new(vec![0.0, 1.0]).unwrap();
    assert_eq!(x_axis.dot(&y_axis), Some(0.0));
    assert_eq!(x_axis.dot(&x_axis), Some(1.0));

    let opposite = EmbeddingVector::try_new(vec![-1.0, 0.0]).unwrap();
    assert_eq!(x_axis.dot(&opposite), Some(-1.0));
}

#[test]
fn vector_accessors_expose_components() {
    let vector = EmbeddingVector::try_new(vec![1.0, 2.0, 3.0]).unwrap();
    assert_eq!(vector.dimensions(), 3);
    assert_eq!(vector.as_slice(), &[1.0, 2.0, 3.0]);
}

#[test]
fn truncate_to_bytes_is_a_no_op_under_the_limit() {
    assert_eq!(truncate_to_bytes("hello", 5), "hello");
    assert_eq!(truncate_to_bytes("hello", 64), "hello");
    assert_eq!(truncate_to_bytes("", 4), "");
}

#[test]
fn truncate_to_bytes_never_splits_a_character() {
    // "é" is two bytes; a cut inside it must back up to the boundary.
    assert_eq!(truncate_to_bytes("aé", 2), "a");
    assert_eq!(truncate_to_bytes("aé", 3), "aé");
    // "🦀" is four bytes.
    assert_eq!(truncate_to_bytes("🦀🦀", 5), "🦀");
    assert_eq!(truncate_to_bytes("🦀", 0), "");
}

async fn embed_one(embedder: &HashEmbedder, text: &str) -> EmbeddingVector {
    let mut vectors = embedder.embed(vec![Arc::from(text)]).await.unwrap();
    assert_eq!(vectors.len(), 1);
    vectors.remove(0)
}

#[tokio::test]
async fn hash_embedder_is_deterministic_across_calls() {
    let embedder = HashEmbedder::try_new(64).unwrap();
    let first = embed_one(&embedder, "the quick brown fox").await;
    let second = embed_one(&embedder, "the quick brown fox").await;
    assert_eq!(first, second);

    // Tokenization lowercases, so case differences do not change the vector.
    let upper = embed_one(&embedder, "The QUICK brown FOX").await;
    assert_eq!(first, upper);
}

#[tokio::test]
async fn hash_embedder_batch_preserves_input_order() {
    let embedder = HashEmbedder::try_new(64).unwrap();
    let alpha = embed_one(&embedder, "alpha alone").await;
    let omega = embed_one(&embedder, "omega instead").await;
    assert_ne!(alpha, omega);

    let batch = embedder
        .embed(vec![Arc::from("omega instead"), Arc::from("alpha alone")])
        .await
        .unwrap();
    assert_eq!(batch, vec![omega, alpha]);
}

#[tokio::test]
async fn hash_embedder_distinct_texts_differ() {
    let embedder = HashEmbedder::try_new(64).unwrap();
    let one = embed_one(&embedder, "remember the deadline").await;
    let other = embed_one(&embedder, "cancel the meeting").await;
    assert_ne!(one, other);
}

#[tokio::test]
async fn hash_embedder_honors_declared_dimensions() {
    for dimensions in [1_usize, 8, 64] {
        let embedder = HashEmbedder::try_new(dimensions).unwrap();
        assert_eq!(embedder.descriptor().dimensions, dimensions);
        let vector = embed_one(&embedder, "some text").await;
        assert_eq!(vector.dimensions(), dimensions);
    }
}

#[tokio::test]
async fn hash_embedder_rejects_empty_input() {
    let embedder = HashEmbedder::try_new(64).unwrap();
    for empty in ["", "   ", "\t\n"] {
        assert_eq!(
            embedder.embed(vec![Arc::from(empty)]).await,
            Err(EmbedError::InvalidInput {
                reason: "embed_input_empty",
            })
        );
    }
    // One invalid text fails the whole batch.
    assert_eq!(
        embedder.embed(vec![Arc::from("fine"), Arc::from("")]).await,
        Err(EmbedError::InvalidInput {
            reason: "embed_input_empty",
        })
    );
    // An empty batch is trivially embedded.
    assert_eq!(embedder.embed(Vec::new()).await, Ok(Vec::new()));
}

#[tokio::test]
async fn hash_embedder_embeds_tokenless_input_via_whole_text_fallback() {
    let embedder = HashEmbedder::try_new(64).unwrap();
    let punctuation = embed_one(&embedder, "!!!").await;
    assert_eq!(punctuation.dimensions(), 64);
    // The fallback hashes the *trimmed* text, so surrounding whitespace is
    // irrelevant.
    let padded = embed_one(&embedder, "  !!! ").await;
    assert_eq!(punctuation, padded);
    // Distinct tokenless texts still land on distinct vectors.
    let other = embed_one(&embedder, "???").await;
    assert_ne!(punctuation, other);
}

#[test]
fn hash_embedder_descriptor_id_is_stable_and_encodes_dimensions() {
    let embedder = HashEmbedder::try_new(64).unwrap();
    let descriptor = embedder.descriptor();
    assert_eq!(descriptor.embedder_id.as_ref(), "embed.hash-v1.64");
    assert_eq!(descriptor.dimensions, 64);
    assert!(descriptor.max_input_bytes > 0);
    assert_eq!(
        HashEmbedder::try_new(8)
            .unwrap()
            .descriptor()
            .embedder_id
            .as_ref(),
        "embed.hash-v1.8"
    );
}

#[test]
fn hash_embedder_rejects_invalid_dimensions() {
    for dimensions in [0, EMBEDDING_MAX_DIMENSIONS + 1] {
        assert_eq!(
            HashEmbedder::try_new(dimensions).err(),
            Some(EmbedError::InvalidInput {
                reason: "embedder_dimensions_invalid",
            })
        );
    }
    assert!(HashEmbedder::try_new(EMBEDDING_MAX_DIMENSIONS).is_ok());
}
