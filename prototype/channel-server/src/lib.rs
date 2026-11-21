//! Domain I/O and explicit App/Server hosting composition.
pub mod app;
#[cfg(target_os = "macos")]
pub mod app_macos;
#[cfg(all(target_os = "windows", feature = "capture"))]
pub mod app_install;
#[cfg(all(target_os = "windows", feature = "capture"))]
pub mod app_windows;
mod backup;
pub mod domain;
pub mod web;

use anyhow::Context;
use domain::Domain;
use piik_channel_core::wire::{HostingMode, RoomCreationPolicy};
use piik_channel_native::peer::server_udp::ServerUdp;
use piik_channel_native::session::{self as native, Config, Hosting, RoomFile};
use std::{
    collections::BTreeSet,
    fs,
    future::Future,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;

pub(crate) fn room_directory(directory: &Path, identity: (uuid::Uuid, uuid::Uuid)) -> PathBuf {
    directory
        .join("rooms")
        .join(identity.0.to_string())
        .join(identity.1.to_string())
}

struct NativeService {
    stop: watch::Sender<bool>,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
}

/// A wildcard listener accepts a family; local clients still need a destination.
pub fn listener_client_address(mut address: std::net::SocketAddr) -> std::net::SocketAddr {
    if address.ip().is_unspecified() {
        address.set_ip(if address.is_ipv4() {
            std::net::Ipv4Addr::LOCALHOST.into()
        } else {
            std::net::Ipv6Addr::LOCALHOST.into()
        });
    }
    address
}

impl NativeService {
    fn start(
        domain: &Domain,
        address: std::net::SocketAddr,
        prefix: &str,
        directory: &Path,
        udp: Arc<ServerUdp>,
        bootstrap: bool,
    ) -> anyhow::Result<Self> {
        let transit_enabled = match std::env::var("PIIK_CHANNEL_SERVICE_TRANSIT") {
            Ok(value) if value == "true" => true,
            Ok(value) if value == "false" => false,
            Err(std::env::VarError::NotPresent) => false,
            _ => anyhow::bail!("PIIK_CHANNEL_SERVICE_TRANSIT must be true or false"),
        };
        let address = listener_client_address(address);
        let directory = room_directory(directory, domain.identity());
        std::fs::create_dir_all(&directory)?;
        let config = Config {
            url: format!("ws://{address}{prefix}/prototype/channel"),
            origin: domain.origin().into(),
            name: domain.owner_name().unwrap_or_else(|| "Piik Server".into()),
            owner_key: Some(domain.owner_key()),
            credential: None,
            server_udp: Some(udp),
        };
        let rooms = if bootstrap {
            [
                ("living", "Living room"),
                ("studio", "Studio"),
                ("quiet", "Quiet corner"),
            ]
            .into_iter()
            .map(|(id, name)| RoomFile {
                path: directory.join(format!("{id}.sqlite")),
                id: id.into(),
                name: name.into(),
                bootstrap: true,
            })
            .collect()
        } else {
            vec![]
        };
        let (commands, requests) = mpsc::channel(64);
        let management = Hosting {
            directory,
            identity: domain.identity(),
            commands: requests,
            ready: watch::channel(None).0,
            desktop: None,
        };
        domain.set_room_commands(Some(commands));
        let (stop, mut stopped) = watch::channel(false);
        let (admission, mut admitted) = mpsc::channel(1);
        let service_domain = domain.clone();
        let task = tokio::spawn(async move {
            let running = native::run_host(
                config,
                rooms,
                Some(management),
                Some(admission),
                async move {
                    let _ = stopped.changed().await;
                },
            );
            tokio::pin!(running);
            loop {
                tokio::select! {
                    result = &mut running => return result,
                    Some((session, acknowledged)) = admitted.recv() => {
                        // Only this owned Native Welcome supplies the service identity.
                        // Its actual socket retirement clears registration in Domain.
                        if service_domain.register_service(session, transit_enabled) { let _ = acknowledged.send(()); }
                    }
                }
            }
        });
        Ok(Self { stop, task })
    }

    async fn close(&mut self) -> anyhow::Result<()> {
        self.stop.send_replace(true);
        (&mut self.task).await?
    }
}

pub fn ice_servers_from_environment() -> anyhow::Result<Vec<serde_json::Value>> {
    match std::env::var("PIIK_CHANNEL_ICE_SERVERS") {
        Ok(value) => Ok(serde_json::from_str(&value)?),
        Err(std::env::VarError::NotPresent) => Ok(vec![serde_json::json!({ "urls": [
            "stun:stun.cloudflare.com:3478", "stun:stun.miwifi.com:3478", "stun:stun.chat.bilibili.com:3478"
        ] })]),
        Err(error) => Err(error.into()),
    }
}

/// Server owns its Native rooms for the explicitly started service lifetime.
pub async fn serve_server(
    listener: tokio::net::TcpListener,
    domain: Domain,
    directory: PathBuf,
    routes: axum::Router,
    udp: Arc<ServerUdp>,
    stop: impl Future<Output = ()>,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(directory.join("rooms"))?;
    for entry in std::fs::read_dir(directory.join("rooms"))? {
        anyhow::ensure!(entry?.path().extension().and_then(|value| value.to_str()) != Some("sqlite"),
            "unsupported_room_storage_layout: preserve rooms/*.sqlite and import supported room backups into the scoped room directory");
    }
    let address = listener.local_addr()?;
    let (commands, mut restores) = mpsc::channel::<domain::RestoreDomain>(1);
    let domain = domain.with_restores(commands);
    let mut hosting = NativeService::start(
        &domain,
        address,
        "",
        &directory,
        udp.clone(),
        domain.was_created(),
    )?;
    let (stop_domain, mut domain_stopped) = watch::channel(false);
    let mut service = tokio::spawn(domain.clone().serve(listener, routes, async move {
        let _ = domain_stopped.changed().await;
    }));
    tokio::pin!(stop);
    let (result, finished_domain) = loop {
        tokio::select! {
            _ = &mut stop => break (hosting.close().await, None),
            result = &mut hosting.task => break (result.map_err(anyhow::Error::from).and_then(|result| result)
                .and_then(|()| Err(anyhow::anyhow!("Native hosting stopped unexpectedly"))), None),
            result = &mut service => {
                let closed = hosting.close().await;
                break (closed.and_then(|()| Err(anyhow::anyhow!("HTTP service stopped unexpectedly"))), Some(result));
            }
            Some(request) = restores.recv() => {
                if request.result.is_closed() { continue; }
                if !domain.is_owner_credential(&request.credential) {
                    let _ = request.result.send(Err("forbidden".into()));
                    continue;
                }
                if let Err(error) = hosting.close().await {
                    let _ = request.result.send(Err(error.to_string()));
                    break (Err(error), None);
                }
                domain.set_room_commands(None);
                let result = domain.restore(request.metadata, &request.credential);
                // A failed durable replacement keeps the old authority and restarts
                // its same saved rooms; neither branch recreates missing histories.
                match NativeService::start(&domain, address, "", &directory, udp.clone(), false) {
                    Ok(next) => hosting = next,
                    Err(error) => {
                        let code = if result.is_ok() {
                            "domain_restored_connection_failed".into()
                        } else {
                            error.to_string()
                        };
                        let _ = request.result.send(Err(code));
                        break (Err(error), None);
                    }
                }
                let _ = request.result.send(result.map_err(|error| error.to_string()));
            }
        }
    };
    // Native departure is acknowledged while the domain service is live.
    let _ = stop_domain.send(true);
    let domain_result = match finished_domain {
        Some(result) => result,
        None => service.await,
    };
    domain_result??;
    result
}

/// One opened domain and the shared room root its native host writes under.
pub struct OpenDomain {
    pub domain: Domain,
    pub directory: PathBuf,
    pub name: String,
}

#[derive(Clone, serde::Serialize)]
pub struct DomainSummary {
    pub id: uuid::Uuid,
    pub name: String,
}

async fn domain_list(
    axum::Extension(domains): axum::Extension<Arc<Vec<DomainSummary>>>,
) -> axum::Json<Vec<DomainSummary>> {
    axum::Json((*domains).clone())
}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
struct DomainRecord {
    id: uuid::Uuid,
    slug: String,
    name: String,
}

#[derive(serde::Deserialize, serde::Serialize)]
struct DomainRegistryFile {
    schema: u8,
    domains: Vec<DomainRecord>,
}

fn domain_slug(name: &str, taken: &mut BTreeSet<String>) -> anyhow::Result<String> {
    let mut slug: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    while slug.contains("--") {
        slug = slug.replace("--", "-");
    }
    let slug = slug.trim_matches('-').to_owned();
    let base = if slug.is_empty() {
        "domain".to_owned()
    } else {
        slug
    };
    let mut candidate = base.clone();
    let mut index = 2;
    while !taken.insert(candidate.clone()) {
        candidate = format!("{base}-{index}");
        index += 1;
    }
    anyhow::ensure!(candidate.len() <= 64, "Domain directory name is too long");
    Ok(candidate)
}

fn load_domain_registry(root: &Path) -> anyhow::Result<DomainRegistryFile> {
    let path = root.join("domains.json");
    if !path.is_file() {
        return Ok(DomainRegistryFile {
            schema: 1,
            domains: vec![],
        });
    }
    let file: DomainRegistryFile = serde_json::from_slice(&fs::read(&path)?)
        .context("Unsupported domain registry file")?;
    anyhow::ensure!(file.schema == 1, "Unsupported domain registry schema");
    Ok(file)
}

fn save_domain_registry(root: &Path, registry: &DomainRegistryFile) -> anyhow::Result<()> {
    fs::create_dir_all(root)?;
    let path = root.join("domains.json");
    let temporary = root.join(format!(".domains-{}.json", uuid::Uuid::new_v4()));
    fs::write(&temporary, serde_json::to_vec_pretty(registry)?)?;
    fs::rename(&temporary, &path)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn open_domain_directory(
    root: &Path,
    domains_root: &Path,
    slug: &str,
    name: String,
    seed: &str,
    origin: String,
    invitation_origin: Option<String>,
    ice_servers: Vec<serde_json::Value>,
    initial_policy: RoomCreationPolicy,
    hosting_mode: HostingMode,
) -> anyhow::Result<OpenDomain> {
    let directory = domains_root.join(slug);
    fs::create_dir_all(&directory)?;
    let domain = Domain::open_with_options(
        &directory,
        seed,
        origin,
        invitation_origin,
        ice_servers,
        initial_policy,
        Some(hosting_mode),
    )?;
    Ok(OpenDomain {
        domain,
        directory: root.to_path_buf(),
        name,
    })
}

/// Open the registered domains plus any `name=seed` additions. The seed only
/// creates a missing domain; an existing checkpoint owns its keys afterwards.
#[allow(clippy::too_many_arguments)]
pub fn open_domains(
    root: &Path,
    additions: &[(String, String)],
    origin: String,
    invitation_origin: Option<String>,
    ice_servers: Vec<serde_json::Value>,
    initial_policy: RoomCreationPolicy,
    hosting_mode: HostingMode,
) -> anyhow::Result<Vec<OpenDomain>> {
    let mut registry = load_domain_registry(root)?;
    let domains_root = root.join("domains");
    fs::create_dir_all(&domains_root)?;
    let mut opened = Vec::new();
    for record in &mut registry.domains {
        let domain = open_domain_directory(
            root,
            &domains_root,
            &record.slug,
            record.name.clone(),
            "",
            origin.clone(),
            invitation_origin.clone(),
            ice_servers.clone(),
            initial_policy,
            hosting_mode,
        )?;
        let (id, name, _) = domain.domain.directory();
        anyhow::ensure!(
            record.id.is_nil() || record.id == id,
            "Domain directory identity changed: {}",
            record.slug
        );
        record.id = id;
        if record.name.trim().is_empty() {
            record.name = name;
        }
        opened.push(domain);
    }
    let mut taken: BTreeSet<String> = registry.domains.iter().map(|record| record.slug.clone()).collect();
    for (name, seed) in additions {
        if registry.domains.iter().any(|record| record.name == *name) {
            continue;
        }
        let slug = domain_slug(name, &mut taken)?;
        let domain = open_domain_directory(
            root,
            &domains_root,
            &slug,
            name.clone(),
            seed,
            origin.clone(),
            invitation_origin.clone(),
            ice_servers.clone(),
            initial_policy,
            hosting_mode,
        )?;
        let (id, checkpoint_name, _) = domain.domain.directory();
        if domain.domain.was_created() && !name.trim().is_empty() {
            domain.domain.set_bootstrap_name(name)?;
        }
        registry.domains.push(DomainRecord {
            id,
            slug,
            name: if name.trim().is_empty() {
                checkpoint_name
            } else {
                name.clone()
            },
        });
        opened.push(domain);
    }
    anyhow::ensure!(!opened.is_empty(), "No domains are configured");
    save_domain_registry(root, &registry)?;
    Ok(opened)
}

/// Serve one listener for several domains; each keeps its own authority, native
/// host and restore owner under its `/d/<id>/` prefix.
pub async fn serve_domains(
    listener: tokio::net::TcpListener,
    domains: Vec<OpenDomain>,
    index: PathBuf,
    routes: axum::Router,
    udp: Arc<ServerUdp>,
    stop: impl Future<Output = ()>,
) -> anyhow::Result<()> {
    let address = listener.local_addr()?;
    let document = crate::web::document::<()>(index);
    let mut summaries = Vec::with_capacity(domains.len());
    let mut controls: Vec<(Domain, watch::Sender<bool>)> = Vec::with_capacity(domains.len());
    let mut tasks = JoinSet::new();
    let mut app = axum::Router::new()
        .route(
            "/healthz",
            axum::routing::get(|| async { "piik-channel-prototype" }),
        )
        .route("/api/server/domains", axum::routing::get(domain_list));
    for OpenDomain { domain, directory, name } in domains {
        let (id, checkpoint_name, _) = domain.directory();
        summaries.push(DomainSummary {
            id,
            name: if name.trim().is_empty() { checkpoint_name } else { name },
        });
        let prefix = format!("/d/{id}");
        app = app
            .route(&prefix, document.clone())
            .route(&format!("{prefix}/"), document.clone())
            .merge(domain.router(&prefix));
        let bootstrap = domain.was_created();
        let (commands, mut restores) = mpsc::channel::<domain::RestoreDomain>(1);
        let domain = domain.with_restores(commands);
        let mut hosting = NativeService::start(&domain, address, &prefix, &directory, udp.clone(), bootstrap)?;
        let (stop_domain, mut domain_stopped) = watch::channel(false);
        let runtime_domain = domain.clone();
        let runtime_prefix = prefix.clone();
        let runtime_directory = directory.clone();
        let runtime_udp = udp.clone();
        tasks.spawn(async move {
            let result = loop {
                tokio::select! {
                    _ = domain_stopped.changed() => break Ok(()),
                    result = &mut hosting.task => break result
                        .map_err(anyhow::Error::from)
                        .and_then(|result| result)
                        .and_then(|()| Err(anyhow::anyhow!("Native hosting stopped unexpectedly"))),
                    Some(request) = restores.recv() => {
                        if request.result.is_closed() { continue; }
                        if !runtime_domain.is_owner_credential(&request.credential) {
                            let _ = request.result.send(Err("forbidden".into()));
                            continue;
                        }
                        if let Err(error) = hosting.close().await {
                            let _ = request.result.send(Err(error.to_string()));
                            break Err(error);
                        }
                        runtime_domain.set_room_commands(None);
                        let restored = runtime_domain.restore(request.metadata, &request.credential);
                        match NativeService::start(&runtime_domain, address, &runtime_prefix, &runtime_directory, runtime_udp.clone(), false) {
                            Ok(next) => hosting = next,
                            Err(error) => {
                                let code = if restored.is_ok() {
                                    "domain_restored_connection_failed".into()
                                } else {
                                    error.to_string()
                                };
                                let _ = request.result.send(Err(code));
                                break Err(error);
                            }
                        }
                        let _ = request.result.send(restored.map_err(|error| error.to_string()));
                    }
                }
            };
            runtime_domain.close_clients();
            let closed = hosting.close().await;
            result.and(closed)
        });
        controls.push((domain, stop_domain));
    }
    let app = app
        .merge(routes)
        .layer(axum::Extension(Arc::new(summaries)));
    let (stop_http, mut http_stopped) = watch::channel(false);
    let mut http = tokio::spawn(crate::web::serve_http(listener, app, async move {
        let _ = http_stopped.changed().await;
    }));
    tokio::pin!(stop);
    let mut result = loop {
        tokio::select! {
            _ = &mut stop => break Ok(()),
            joined = tasks.join_next() => break match joined {
                Some(Ok(Ok(()))) => Err(anyhow::anyhow!("A domain runtime stopped unexpectedly")),
                Some(Ok(Err(error))) => Err(error),
                Some(Err(error)) => Err(anyhow::Error::from(error)),
                None => Err(anyhow::anyhow!("All domain runtimes stopped")),
            },
            http_result = &mut http => break match http_result {
                Ok(Ok(())) => Err(anyhow::anyhow!("HTTP service stopped unexpectedly")),
                Ok(Err(error)) => Err(anyhow::Error::from(error)),
                Err(error) => Err(anyhow::Error::from(error)),
            },
        }
    };
    for (domain, stop_domain) in &controls {
        domain.close_clients();
        let _ = stop_domain.send(true);
    }
    while tasks.join_next().await.is_some() {}
    let _ = stop_http.send(true);
    match http.await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            if result.is_ok() {
                result = Err(anyhow::Error::from(error));
            }
        }
        Err(error) => {
            if result.is_ok() {
                result = Err(anyhow::Error::from(error));
            }
        }
    }
    result
}

#[cfg(test)]
mod domain_tests {
    use super::*;

    #[test]
    fn registry_reopens_same_domains_with_stable_identity_and_names() {
        let root = std::env::temp_dir().join(format!("piik-domains-{}", uuid::Uuid::new_v4()));
        let origin = "http://127.0.0.1:18080".to_owned();
        let additions = vec![
            ("Living".to_owned(), "seed-living".to_owned()),
            ("Studio".to_owned(), "seed-studio".to_owned()),
        ];
        let first = open_domains(
            &root,
            &additions,
            origin.clone(),
            None,
            vec![],
            RoomCreationPolicy::OwnerOnly,
            HostingMode::Hosted,
        )
        .unwrap();
        let identities: Vec<_> = first
            .iter()
            .map(|domain| (domain.domain.directory().0, domain.name.clone()))
            .collect();
        assert_eq!(identities.len(), 2);
        assert_eq!(identities[0].1, "Living");
        assert_eq!(identities[1].1, "Studio");
        for domain in &first {
            assert_eq!(domain.domain.directory().1, domain.name);
        }
        drop(first);
        let reopened = open_domains(
            &root,
            &[],
            origin,
            None,
            vec![],
            RoomCreationPolicy::OwnerOnly,
            HostingMode::Hosted,
        )
        .unwrap();
        let restored: Vec<_> = reopened
            .iter()
            .map(|domain| (domain.domain.directory().0, domain.name.clone()))
            .collect();
        assert_eq!(restored, identities);
        assert!(root.join("domains/living/channel.sqlite").is_file());
        assert!(root.join("domains/studio/channel.sqlite").is_file());
        drop(reopened);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
