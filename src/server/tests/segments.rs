//! The segment route: a segment's stored bytes, the same on every request,
//! cached for a year and never sent damaged; a request naming its tag
//! answered `304`; and the hashes it refuses.

use reqwest::header::{ETAG, IF_NONE_MATCH};

use super::*;
use crate::protocol::segments::{SEGMENT_CACHE_CONTROL, SEGMENT_MEDIA_TYPE};
use crate::storage::segments;

/// A bed with one packed window of the Spot market, and that segment's
/// name.
fn packed() -> (Bed, SegmentHash) {
    let bed = Bed::new();
    let spot = market(Category::Spot);
    let candles: Vec<Candle> = (0..3)
        .map(|minute| candle(T + minute * MINUTE, "67012.34000000", Some(9)))
        .chain([candle(T + DAY, "1", None)])
        .collect();
    bed.store
        .upsert_candles(&spot, &candles)
        .expect("the candles");
    bed.store.refresh_segments(&spot).expect("a refresh");
    let plan = bed
        .store
        .resolve_segments(&spot, T, T + DAY)
        .expect("a plan");
    let hash = plan.segments[0].hash;
    (bed, hash)
}

async fn get_segment(address: SocketAddr, name: &str, if_none_match: &[&str]) -> reqwest::Response {
    let mut request = client().get(format!("http://{address}/v1/segments/{name}"));
    for tag in if_none_match {
        request = request.header(IF_NONE_MATCH, *tag);
    }
    request.send().await.expect("a response")
}

#[tokio::test]
async fn a_segment_is_its_stored_bytes_on_every_request_cached_for_a_year() {
    let (bed, hash) = packed();
    let stored = bed
        .store
        .load_segment(&hash)
        .expect("a read")
        .expect("the segment");
    let address = start(bed.history()).await;
    let tag = format!("\"{hash}\"");
    for _ in 0..2 {
        let response = get_segment(address, &hash.to_string(), &[]).await;
        assert_eq!(response.status(), 200);
        assert_eq!(header(&response, CONTENT_TYPE), Some(SEGMENT_MEDIA_TYPE));
        assert_eq!(
            header(&response, CACHE_CONTROL),
            Some(SEGMENT_CACHE_CONTROL)
        );
        assert_eq!(header(&response, ETAG), Some(tag.as_str()));
        assert_eq!(header(&response, "content-encoding"), None);
        assert_eq!(
            response.bytes().await.expect("the bytes").as_ref(),
            stored.as_slice()
        );
    }
    let candles = segments::open(&stored, &hash).expect("the stored bytes match their name");
    assert_eq!(candles.len(), 3);

    let head = client()
        .head(format!("http://{address}/v1/segments/{hash}"))
        .send()
        .await
        .expect("a response");
    assert_eq!(head.status(), 200);
    assert_eq!(header(&head, ETAG), Some(tag.as_str()));
    assert_eq!(head.bytes().await.expect("no body").len(), 0);
}

#[tokio::test]
async fn a_request_that_names_the_segments_tag_is_answered_not_modified() {
    let (bed, hash) = packed();
    let address = start(bed.history()).await;
    let tag = format!("\"{hash}\"");
    let weak = format!("W/{tag}");
    let listed = format!("\"sha256:{}\", {tag}", "0".repeat(64));
    for names in [
        &[tag.as_str()][..],
        &[weak.as_str()],
        &[listed.as_str()],
        &["*"],
        &["\"x\"", tag.as_str()],
    ] {
        let response = get_segment(address, &hash.to_string(), names).await;
        assert_eq!(response.status(), 304, "{names:?}");
        assert_eq!(header(&response, ETAG), Some(tag.as_str()));
        assert_eq!(
            header(&response, CACHE_CONTROL),
            Some(SEGMENT_CACHE_CONTROL)
        );
        assert_eq!(response.bytes().await.expect("no body").len(), 0);
    }
    let other = format!("\"sha256:{}\"", "0".repeat(64));
    for names in [&[other.as_str()][..], &[hash.to_string().as_str()]] {
        let response = get_segment(address, &hash.to_string(), names).await;
        assert_eq!(
            response.status(),
            200,
            "{names:?}: the tag is the hash in quotes"
        );
    }
}

#[tokio::test]
async fn a_hash_the_server_does_not_hold_or_cannot_read_is_refused() {
    let (bed, hash) = packed();
    let address = start(bed.history()).await;

    let unknown = format!("sha256:{}", "0".repeat(64));
    let (status, code, message) = refusal(get_segment(address, &unknown, &[]).await).await;
    assert_eq!((status, code.as_str()), (404, "not_found"));
    assert_eq!(message, format!("the server holds no segment {unknown}"));
    // Not even a cache's tag makes it exist.
    let tagged = format!("\"{unknown}\"");
    let (status, ..) = refusal(get_segment(address, &unknown, &["*", &tagged]).await).await;
    assert_eq!(status, 404);

    let uppercase = hash
        .to_string()
        .to_uppercase()
        .replace("SHA256:", "sha256:");
    for name in ["e3b0c442", "sha256:xyz", uppercase.as_str(), "sha256%3A00"] {
        let (status, code, message) = refusal(get_segment(address, name, &[]).await).await;
        assert_eq!((status, code.as_str()), (400, "invalid_request"), "{name}");
        assert!(
            message.contains("invalid content hash"),
            "{name}: {message}"
        );
    }
}

#[tokio::test]
async fn a_stored_segment_that_does_not_match_its_name_is_never_sent() {
    let (bed, hash) = packed();
    let address = start(bed.history()).await;
    let other = segments::compress(&segments::encode(&[candle(T, "1", None)]).expect("a payload"))
        .expect("a frame");
    let truncated = {
        let stored = bed
            .store
            .load_segment(&hash)
            .expect("a read")
            .expect("the segment");
        stored[..stored.len() / 2].to_vec()
    };
    for bytes in [other, truncated, b"not zstd".to_vec()] {
        bed.connection()
            .execute(
                "UPDATE segments SET data = ?1 WHERE hash = ?2",
                rusqlite::params![bytes, hash.to_string()],
            )
            .expect("the damage");
        let (status, code, message) =
            refusal(get_segment(address, &hash.to_string(), &[]).await).await;
        assert_eq!((status, code.as_str()), (500, "internal"));
        assert_eq!(
            message,
            format!("the server's copy of segment {hash} is damaged")
        );
        // A client that holds the bytes already is told so: nothing is sent.
        let tag = format!("\"{hash}\"");
        let response = get_segment(address, &hash.to_string(), &[&tag]).await;
        assert_eq!(response.status(), 304);
    }
}
