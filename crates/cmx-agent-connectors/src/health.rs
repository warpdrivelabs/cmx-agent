//! 连接器健康探测。打服务的 `/_mon/tech-stats`（**无需 auth**，本机实测三服务皆有），
//! 解出服务名 + uptime，作为面板的 live 状态。失败 = 离线（不报错，返回 offline 状态）。

use crate::client::CmxServiceClient;

/// 一个连接器的 live 健康状态。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ConnectorStatus {
    pub online: bool,
    /// 服务自报名（如 "cmx-flow 流程引擎"）；离线时为 None。
    pub service_name: Option<String>,
    pub uptime_secs: Option<u64>,
    pub latency_ms: Option<u64>,
    /// 离线原因（诊断用）。
    pub detail: Option<String>,
}

impl ConnectorStatus {
    fn offline(detail: impl Into<String>) -> Self {
        Self {
            online: false,
            service_name: None,
            uptime_secs: None,
            latency_ms: None,
            detail: Some(detail.into()),
        }
    }
}

/// 探测一个服务的健康。打 `/_mon/tech-stats`，解 `data.service.name` 与 `data.requests.overview.uptimeSecs`。
pub async fn probe(client: &CmxServiceClient) -> ConnectorStatus {
    let started = std::time::Instant::now();
    match client.get_raw("/_mon/tech-stats").await {
        Ok(body) => {
            let latency = started.elapsed().as_millis() as u64;
            // tech-stats 也是 {code,msg,data} 信封
            let data = body.get("data").unwrap_or(&body);
            let service_name = data
                .get("service")
                .and_then(|s| s.get("name"))
                .and_then(|n| n.as_str())
                .map(|s| s.to_string());
            let uptime = data
                .get("requests")
                .and_then(|r| r.get("overview"))
                .and_then(|o| o.get("uptimeSecs"))
                .and_then(|u| u.as_u64());
            ConnectorStatus {
                online: true,
                service_name,
                uptime_secs: uptime,
                latency_ms: Some(latency),
                detail: None,
            }
        }
        Err(e) => ConnectorStatus::offline(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_status_has_detail() {
        let s = ConnectorStatus::offline("connection refused");
        assert!(!s.online);
        assert_eq!(s.detail.as_deref(), Some("connection refused"));
        assert!(s.service_name.is_none());
    }
}
