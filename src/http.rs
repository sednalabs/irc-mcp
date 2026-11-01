//! Hosted boundary: exact authority/origin checks, toolkit OAuth, then local policy.
//! Principal material is created only after consuming the current request witness.
use crate::{
    config::Config,
    domain::Failure,
    mcp::IrcMcp,
    service::{epoch, Principal, Service},
};
use axum::{
    extract::{Request, State},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{any, get},
    Router,
};
use mcp_toolkit::server::apps_http::mirror_apps_tool_security_schemes;
use mcp_toolkit::rmcp::transport::streamable_http_server::StreamableHttpServerConfig;
use mcp_toolkit::server::{
    auth::{AuthSurfaceBuilder, IssuerEntry},
    http::{handle_mcp, LocalMcpHttpRuntimeBuilder},
};
use mcp_toolkit_auth::{
    consume_verified_auth_surface_request_from_request, discover_oidc_metadata,
    surface::AuthorizationServerMetadataSource, AuthConfig, AuthMode, Authenticator,
};
use std::{collections::BTreeSet, sync::Arc};
use tokio_util::sync::CancellationToken;

type BuildResult<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
#[derive(Clone)]
struct Gate {
    cfg: Arc<Config>,
    auth: Arc<Authenticator>,
}
#[derive(Clone)]
struct Edge {
    hosts: BTreeSet<String>,
    origins: BTreeSet<String>,
}
pub async fn build(app: Arc<Service>, stop: CancellationToken) -> BuildResult<Router> {
    let cfg = app.cfg.clone();
    let metadata = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        discover_oidc_metadata(&cfg.issuer, None),
    )
    .await??;
    if !chatgpt_oauth_metadata_ready(
        metadata.code_challenge_methods_supported.as_deref(),
        metadata.token_endpoint_auth_methods_supported.as_deref(),
    ) {
        return Err(
            "OAuth issuer must advertise PKCE S256 and a supported token authentication method"
                .into(),
        );
    }
    let auth = Arc::new(Authenticator::new(AuthConfig {
        mode: AuthMode::Jwks,
        strict_oauth: true,
        jwks_url: Some(metadata.jwks_uri.clone()),
        issuer: Some(cfg.issuer.clone()),
        audience: Some(cfg.resource_url()),
        required_scopes: vec!["irc.read".into()],
        actor_claim: "sub".into(),
        ..AuthConfig::default()
    })?);
    let entry = IssuerEntry::from_metadata_source(
        "/mcp",
        AuthorizationServerMetadataSource::OidcDiscovery(metadata),
        "irc",
        vec!["irc.read".into(), "irc.write".into()],
        cfg.allowed_client_ids.iter().cloned().collect(),
        auth.clone(),
        Some(cfg.resource_url()),
    )?;
    let auth_layer = AuthSurfaceBuilder::single_issuer(cfg.public_base_url.clone(), entry)
        .public_path("/health")
        .build()?;
    let host = cfg.authority()?;
    let hosts = vec![host, cfg.bind.to_string()];
    let server = IrcMcp::new(app, stop.clone())?;
    let runtime = LocalMcpHttpRuntimeBuilder::new()
        .server_config(StreamableHttpServerConfig::default().with_json_response(true))
        .allowed_hosts(hosts.clone())
        .allowed_origins(cfg.allowed_origins.clone())
        .max_request_body_bytes(16 * 1024)
        .cancellation_token(stop)
        .build(move || Ok(server.clone()));
    let state = runtime.into_state(true);
    let router = Router::new()
        .route("/mcp", any(handle_mcp::<IrcMcp>))
        .route("/mcp/", any(handle_mcp::<IrcMcp>))
        .route("/health", get(|| async { "alive" }))
        .layer(middleware::from_fn(mirror_apps_tool_security_schemes))
        .with_state(state)
        .layer(middleware::from_fn_with_state(
            Gate {
                cfg: cfg.clone(),
                auth,
            },
            subject_gate,
        ))
        .layer(auth_layer)
        .layer(middleware::from_fn_with_state(
            Edge {
                hosts: hosts.into_iter().collect(),
                origins: cfg.allowed_origins.iter().cloned().collect(),
            },
            edge_gate,
        ));
    Ok(router)
}
fn chatgpt_oauth_metadata_ready(pkce: Option<&[String]>, token_methods: Option<&[String]>) -> bool {
    pkce.is_some_and(|m| m.iter().any(|s| s == "S256"))
        && token_methods.is_some_and(|m| {
            m.iter().any(|s| {
                matches!(
                    s.as_str(),
                    "none" | "private_key_jwt" | "client_secret_post" | "client_secret_basic"
                )
            })
        })
}
async fn edge_gate(State(edge): State<Edge>, request: Request, next: Next) -> Response {
    let bad = || {
        (
            http::StatusCode::FORBIDDEN,
            "Invalid request authority or origin.",
        )
            .into_response()
    };
    if request.headers().get_all(http::header::HOST).iter().count() != 1 {
        return bad();
    }
    let Some(host) = request
        .headers()
        .get(http::header::HOST)
        .and_then(|h| h.to_str().ok())
    else {
        return bad();
    };
    if !edge.hosts.contains(&host.to_ascii_lowercase()) {
        return bad();
    }
    if request
        .uri()
        .authority()
        .is_some_and(|a| !a.as_str().eq_ignore_ascii_case(host))
    {
        return bad();
    }
    if request
        .headers()
        .get_all(http::header::ORIGIN)
        .iter()
        .count()
        > 1
    {
        return bad();
    }
    if let Some(origin) = request.headers().get(http::header::ORIGIN) {
        let Ok(origin) = origin.to_str() else {
            return bad();
        };
        if !edge.origins.contains(origin) {
            return bad();
        }
    }
    next.run(request).await
}
async fn subject_gate(State(gate): State<Gate>, mut request: Request, next: Next) -> Response {
    if request.uri().path() == "/health" {
        return next.run(request).await;
    }
    if !matches!(request.uri().path(), "/mcp" | "/mcp/") {
        return http::StatusCode::NOT_FOUND.into_response();
    }
    let Some(verified) = consume_verified_auth_surface_request_from_request(&request, &gate.auth)
    else {
        return (
            http::StatusCode::UNAUTHORIZED,
            "Current authenticated request evidence is missing.",
        )
            .into_response();
    };
    let Some(auth) = verified.auth() else {
        return http::StatusCode::UNAUTHORIZED.into_response();
    };
    let Some(subject) = auth.subject.as_deref() else {
        return http::StatusCode::FORBIDDEN.into_response();
    };
    if subject != gate.cfg.subject {
        return http::StatusCode::FORBIDDEN.into_response();
    }
    let Some(expires) = auth.claims.get("exp").and_then(serde_json::Value::as_u64) else {
        return http::StatusCode::UNAUTHORIZED.into_response();
    };
    if epoch().map_or(true, |now| expires <= now) {
        return http::StatusCode::UNAUTHORIZED.into_response();
    }
    let principal = Principal {
        subject: subject.to_owned(),
        scopes: auth.scopes.iter().cloned().collect(),
        expires,
    };
    if principal.require("irc.read").is_err() {
        return http::StatusCode::FORBIDDEN.into_response();
    }
    request.extensions_mut().insert(principal);
    next.run(request).await
}
/// Helper for the binary: never put upstream errors or credentials in an HTTP response.
pub fn startup_failure() -> Failure {
    Failure::new(
        "startup",
        "Startup failed; inspect configuration and trusted upstream availability.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;
    #[test]
    fn oauth_host_requires_pkce_and_token_method_metadata() {
        let s256 = ["S256".to_owned()];
        let plain = ["plain".to_owned()];
        let public = ["none".to_owned()];
        assert!(chatgpt_oauth_metadata_ready(Some(&s256), Some(&public)));
        assert!(!chatgpt_oauth_metadata_ready(Some(&plain), Some(&public)));
        assert!(!chatgpt_oauth_metadata_ready(Some(&s256), None));
    }
    #[tokio::test]
    async fn denies_foreign_host_and_origin_before_inner_service() {
        let app =
            Router::new()
                .route("/", get(|| async { "ok" }))
                .layer(middleware::from_fn_with_state(
                    Edge {
                        hosts: BTreeSet::from(["mcp.example.test".into()]),
                        origins: BTreeSet::new(),
                    },
                    edge_gate,
                ));
        for (host, origin) in [
            ("evil.example", None),
            ("mcp.example.test", Some("null")),
            ("mcp.example.test", Some("https://evil.example")),
        ] {
            let mut b = http::Request::builder().uri("/").header("host", host);
            if let Some(o) = origin {
                b = b.header("origin", o);
            }
            let r = app
                .clone()
                .oneshot(b.body(axum::body::Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(r.status(), http::StatusCode::FORBIDDEN);
        }
        let r = app
            .oneshot(
                http::Request::builder()
                    .uri("/")
                    .header("host", "mcp.example.test")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), http::StatusCode::OK);
    }
}
