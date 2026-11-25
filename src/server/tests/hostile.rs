//! Requests made to do harm: an upload whose entries would leave the place
//! a bundle is written to, or are no files at all, refused with the
//! bundle's own refusal and none of it stored; an upload sent packed, read
//! as the bytes sent and never unpacked; and a path that climbs, which
//! names no route and no file, since no route reads one: a bundle, a
//! segment and a result are rows of the store, named by a hash or a job.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::jobs::{
    Admitting, MANIFEST, STRATEGY, bundle, jobs_of, name_of, put_bundle, runtime, serve_jobs,
};
use super::*;

/// One entry of an archive as a client that means harm writes it: any
/// name, any type, and a link's target where it is one.
fn entry(name: &str, data: &str, typeflag: u8, linkname: &str) -> (String, Vec<u8>) {
    let mut block = [0_u8; 512];
    block[..name.len()].copy_from_slice(name.as_bytes());
    block[100..108].copy_from_slice(b"0000644\0");
    block[108..116].copy_from_slice(b"0000000\0");
    block[116..124].copy_from_slice(b"0000000\0");
    block[124..136].copy_from_slice(format!("{:011o}\0", data.len()).as_bytes());
    block[136..148].copy_from_slice(b"00000000000\0");
    block[148..156].copy_from_slice(b"        ");
    block[156] = typeflag;
    block[157..157 + linkname.len()].copy_from_slice(linkname.as_bytes());
    block[257..263].copy_from_slice(b"ustar\0");
    block[263..265].copy_from_slice(b"00");
    let sum: u32 = block.iter().map(|&byte| u32::from(byte)).sum();
    block[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    let mut written = block.to_vec();
    written.extend_from_slice(data.as_bytes());
    written.resize(written.len().div_ceil(512) * 512, 0);
    (name.to_string(), written)
}

/// The spec's smallest bundle with `more` beside it, its entries sorted by
/// name, as a canonical archive's are.
fn beside(more: (String, Vec<u8>)) -> Vec<u8> {
    let mut entries = vec![
        entry("quantforge.toml", MANIFEST, b'0', ""),
        entry("strategy.py", STRATEGY, b'0', ""),
        more,
    ];
    entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut tar: Vec<u8> = entries
        .into_iter()
        .flat_map(|(_, written)| written)
        .collect();
    tar.resize(tar.len() + 1024, 0);
    tar
}

/// How many bundles the bed's store holds.
fn stored(bed: &Bed) -> i64 {
    bed.connection()
        .query_row("SELECT count(*) FROM bundles", [], |row| row.get(0))
        .expect("a count")
}

#[tokio::test]
async fn an_upload_whose_entries_would_leave_a_bundle_is_refused_and_none_of_it_is_stored() {
    let bed = Bed::new();
    let address = serve_jobs(&bed, jobs_of(&bed, Admitting(runtime("3.12")), 100)).await;
    let cases = [
        (
            "a name that climbs out",
            entry("../escape.py", "x", b'0', ""),
            "\"../escape.py\" is not a bundle path: it has a . or .. component",
        ),
        (
            "a name that climbs out of a directory of the bundle's",
            entry("pkg/../../escape.py", "x", b'0', ""),
            "\"pkg/../../escape.py\" is not a bundle path: it has a . or .. component",
        ),
        (
            "a name from the root",
            entry("/etc/cron.d/job.txt", "x", b'0', ""),
            "\"/etc/cron.d/job.txt\" is not a bundle path: it must be relative to the bundle",
        ),
        (
            "a name as Windows reads one",
            entry("..\\escape.py", "x", b'0', ""),
            "\"..\\\\escape.py\" is not a bundle path: separate directories with /, not \\",
        ),
        (
            "a symlink out of the bundle",
            entry("linked.py", "", b'2', "/etc/passwd"),
            "entry 1 (\"linked.py\"): its type is '2', but a bundle holds only regular files",
        ),
        (
            "a hard link to a file outside",
            entry("linked.py", "", b'1', "../../etc/passwd"),
            "entry 1 (\"linked.py\"): its type is '1', but a bundle holds only regular files",
        ),
        (
            "a directory",
            entry("pkg", "", b'5', ""),
            "entry 1 (\"pkg\"): its type is '5', but a bundle holds only regular files",
        ),
        (
            "a device",
            entry("tty.txt", "", b'3', ""),
            "its type is '3', but a bundle holds only regular files",
        ),
    ];
    for (case, more, expected) in cases {
        let tar = beside(more);
        let name = name_of(&tar).to_string();
        let response = put_bundle(address, &name, Some("application/x-tar"), tar).await;
        let (status, code, message) = refusal(response).await;
        assert_eq!((status, code.as_str()), (400, "invalid_request"), "{case}");
        assert!(message.contains(expected), "{case}: {message}");
    }
    assert_eq!(stored(&bed), 0, "a refused upload was stored");
}

#[tokio::test]
async fn an_upload_sent_packed_is_read_as_the_bytes_sent_and_never_unpacked() {
    let bed = Bed::new();
    let address = serve_jobs(&bed, jobs_of(&bed, Admitting(runtime("3.12")), 100)).await;
    let tar = bundle().canonical_tar();
    let packed = zstd::bulk::compress(&tar, 3).expect("packed");
    // Megabytes of nothing in a few hundred bytes, as a bomb is packed.
    let bomb = zstd::bulk::compress(&vec![0_u8; 8 << 20], 19).expect("packed");
    for (body, encoding) in [(&packed, "zstd"), (&bomb, "zstd"), (&packed, "gzip")] {
        let send = |name: String| {
            client()
                .put(format!("http://{address}/v1/bundles/{name}"))
                .header(CONTENT_TYPE, "application/x-tar")
                .header("Content-Encoding", encoding)
                .body(body.clone())
                .send()
        };
        // Under the name of what was sent: no canonical tar.
        let response = send(name_of(body).to_string()).await.expect("a response");
        let (status, code, message) = refusal(response).await;
        assert_eq!((status, code.as_str()), (400, "invalid_request"));
        assert!(
            message.contains("the archive is not the canonical tar of its files"),
            "{message}"
        );
        // Under the name of what unpacking it would give: the server hashes
        // what it received, and that is another name.
        let response = send(name_of(&tar).to_string()).await.expect("a response");
        let (status, _, message) = refusal(response).await;
        assert_eq!(status, 400);
        assert!(
            message.starts_with(&format!("the upload hashes to {}", name_of(body))),
            "{message}"
        );
    }
    assert_eq!(stored(&bed), 0, "a packed upload was stored");
}

/// The status and the body of `target` asked for with `method`, the request
/// written as it stands: a client's library would tidy a path that climbs
/// before sending it.
async fn asked(address: SocketAddr, method: &str, target: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(address).await.expect("a connection");
    let request = format!(
        "{method} {target} HTTP/1.1\r\nHost: {address}\r\nContent-Length: 0\r\nConnection: \
         close\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await.expect("sent");
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer).await.expect("an answer");
    let answer = String::from_utf8_lossy(&answer).into_owned();
    let status = answer
        .split(' ')
        .nth(1)
        .and_then(|status| status.parse().ok())
        .unwrap_or_else(|| panic!("no status in {answer:?}"));
    let body = answer
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_string())
        .unwrap_or_default();
    (status, body)
}

#[tokio::test]
async fn a_path_that_climbs_names_no_route_and_no_file() {
    let bed = Bed::new();
    let address = serve_jobs(&bed, jobs_of(&bed, Admitting(runtime("3.12")), 100)).await;
    let job = "0123456789abcdef0123456789abcdef";
    let hash = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    let cases = [
        ("GET", "/../../../../etc/passwd".to_string(), 404),
        ("GET", "/v1/../../etc/passwd".to_string(), 404),
        ("GET", "/v1/backtests/../../../etc/passwd".to_string(), 404),
        (
            "GET",
            "/v1/backtests/%2e%2e/%2e%2e/etc/passwd".to_string(),
            404,
        ),
        ("GET", "//etc/passwd".to_string(), 404),
        (
            "GET",
            "/v1/segments/..%2f..%2fmarket.sqlite".to_string(),
            400,
        ),
        (
            "GET",
            format!("/v1/segments/{hash}/../../market.sqlite"),
            404,
        ),
        (
            "GET",
            format!("/v1/backtests/{job}/result/..%2f..%2f..%2fetc%2fpasswd"),
            404,
        ),
        ("GET", format!("/v1/backtests/{job}/result/%2e%2e"), 404),
        (
            "GET",
            format!("/v1/backtests/{job}/result/metrics.json%00.txt"),
            404,
        ),
        (
            "GET",
            "/v1/backtests/..%2f..%2fetc%2fpasswd/result/metrics.json".to_string(),
            400,
        ),
        ("PUT", "/v1/bundles/..%2f..%2fescape.py".to_string(), 400),
        ("PUT", format!("/v1/bundles/{hash}/../../escape.py"), 404),
        ("POST", "/v1/backtests/..%2f..%2fx/cancel".to_string(), 400),
    ];
    for (method, target, expected) in cases {
        let (status, body) = asked(address, method, &target).await;
        assert_eq!(status, expected, "{method} {target}: {body}");
        let envelope: Value = serde_json::from_str(&body)
            .unwrap_or_else(|_| panic!("{method} {target}: no error envelope in {body:?}"));
        let code = envelope["error"]["code"].as_str().expect("a code");
        assert!(
            ["not_found", "invalid_request"].contains(&code),
            "{method} {target}: {body}"
        );
    }
    assert_eq!(stored(&bed), 0);
}
