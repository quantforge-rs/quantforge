//! A succeeded job's result, downloaded and checked before it is read.

use sha2::{Digest, Sha256};

use super::{ClientError, RemoteClient};
use crate::protocol::ContentHash;
use crate::protocol::jobs::{JobId, JobResult};
use crate::protocol::results::{RESULT_FORMAT, ResultKind, TradeRow, decode_trades};

/// What a succeeded job produced, as the client checked it: the job's
/// result as the server listed it, and its closed trades, decoded from a
/// `trades.arrow` whose size and hash are the listed ones.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteResult {
    /// The metrics, and where each file is.
    pub result: JobResult,
    /// Every closed trade, in the order they closed.
    pub trades: Vec<TradeRow>,
}

impl RemoteClient {
    /// A succeeded job's result: its metrics, and its trades, downloaded and
    /// checked against the size and hash the result lists before they are
    /// decoded. The equity curve is left on the server.
    pub async fn result(&self, job: &JobId) -> Result<RemoteResult, ClientError> {
        let address = self.route(&format!("backtests/{job}/result"));
        let result: JobResult = self.patiently(|| self.http.get(address.clone())).await?;
        if result.format != RESULT_FORMAT {
            return Err(self.damaged(format!(
                "a result of format {}, and this build reads format {RESULT_FORMAT}",
                result.format
            )));
        }
        let kind = ResultKind::Trades;
        let link = result
            .results
            .iter()
            .find(|link| link.name == kind.name())
            .ok_or_else(|| self.damaged(format!("a result that lists no {}", kind.name())))?;
        // Built from the base rather than taken from `link.path`, which is
        // the path on the server itself, without a proxy's prefix.
        let file = self.route(&format!("backtests/{job}/result/{}", kind.file_name()));
        let response = self.send_patiently(|| self.http.get(file.clone())).await?;
        let bytes = self.bytes(response).await?;
        let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        let hash = ContentHash::sha256(Sha256::digest(&bytes).into());
        if size != link.size_bytes || hash != link.hash {
            return Err(self.damaged(format!(
                "{} of {size} bytes hashing to {hash}, where the result lists {} bytes hashing \
                 to {}",
                kind.file_name(),
                link.size_bytes,
                link.hash
            )));
        }
        let trades = decode_trades(&bytes).map_err(|error| {
            self.damaged(format!("{} it cannot read: {error}", kind.file_name()))
        })?;
        if u64::try_from(trades.len()).unwrap_or(u64::MAX) != result.metrics.trade_count {
            return Err(self.damaged(format!(
                "{} trades, where the metrics count {}",
                trades.len(),
                result.metrics.trade_count
            )));
        }
        Ok(RemoteResult { result, trades })
    }
}

#[cfg(test)]
mod tests;
