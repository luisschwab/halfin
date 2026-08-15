// SPDX-License-Identifier: MIT OR Apache-2.0

//! Start and control a `florestad` [`Node`] process.
//!
//! [`FlorestaD`] starts the Floresta daemon with an isolated data directory.
//! It assigns local JSON-RPC and Electrum ports.
//! Use it as an outbound peer because this version does not accept inbound peer connections.
//!
//! [`Node`]: crate::node::Node

use core::net::SocketAddr;
use core::net::SocketAddrV4;
use std::path::Path;
use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::process::ExitStatus;
use std::process::Stdio;
use std::thread::sleep;
use std::time::Duration;
use std::time::Instant;

use corepc_client::bitcoin::BlockHash;
use corepc_client::bitcoin::Network;
use electrum_client::ElectrumApi;
use electrum_client::raw_client::ElectrumPlaintextStream;
use electrum_client::raw_client::RawClient;
use miniscript::Descriptor;
use miniscript::DescriptorPublicKey;
use tracing::debug;

use self::client_versions::Client;
use crate::CONNECTION_INTERVAL;
use crate::CONNECTION_TIMEOUT;
use crate::DataDir;
use crate::Error;
use crate::IPV4_LOCALHOST;
use crate::SPAWN_ATTEMPTS;
use crate::SPAWN_INTERVAL;
use crate::find_conflicting_argument;
use crate::get_available_port;
use crate::init_data_dir;
use crate::node::Node;
use crate::node::NodeArgs;
use crate::node::NodeClientError;
use crate::node::NodeError;
use crate::node::PruneMode;
use crate::pipe_to_tracing;

/// Version-specific JSON-RPC client aliases for the bundled `florestad`.
mod client_versions;
/// Bundled `florestad` version metadata.
mod versions;

/// Return the path to the downloaded `florestad` binary.
///
/// At compile time, `build.rs` downloads and extracts the binary.
/// It stores the binary path in `HALFIN_FLORESTAD_PATH`.
///
/// # Errors
///
/// Returns [`Error::BinaryNotFound`] if the compiled-in binary path does not exist.
pub fn get_florestad_path() -> Result<PathBuf, Error> {
    #[allow(unused_mut)]
    let mut bin_path = PathBuf::from(option_env!("HALFIN_FLORESTAD_PATH").unwrap_or(""));

    #[cfg(target_os = "windows")]
    if bin_path.extension().is_none() {
        bin_path.set_extension("exe");
    }

    let bin_name = FlorestaD::get_bin_name().to_string();
    match bin_path.exists() {
        true => Ok(bin_path),
        false => Err(Error::BinaryNotFound((bin_name, bin_path))),
    }
}

/// Arguments specific to `florestad`.
#[derive(Debug, PartialEq, Eq, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct FlorestaDArgs {
    /// Enables peer discovery through DNS seeds.
    pub dns_seeds: bool,
    /// Permits automatic peer connections to use `P2Pv1` if `P2Pv2` fails.
    ///
    /// This field does not select the transport for [`FlorestaD::add_peer`].
    /// That method uses [`NodeArgs::v2_transport`].
    pub allow_v1_fallback: bool,
    /// Uses the Floresta assume-Utreexo snapshot.
    pub assume_utreexo: bool,
    /// Validates skipped blocks in the background.
    pub backfill: bool,
    /// Output descriptors for transactions that Floresta indexes.
    pub wallet_descriptors: Vec<Descriptor<DescriptorPublicKey>>,
}

/// Configuration for a [`FlorestaD`] instance.
///
/// Set only `tmpdir` or `staticdir`.
/// By default, each [`Node`] uses a new temporary directory that [`Drop`] deletes.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct FlorestaDConf {
    /// Arguments shared with other [`Node`] implementations.
    pub args: NodeArgs,
    /// Arguments specific to `florestad`.
    pub florestad_args: FlorestaDArgs,
    /// Extra CLI arguments sent unchanged to `florestad`.
    ///
    /// Do not duplicate arguments that typed configuration or `halfin` controls.
    /// A duplicate argument returns [`NodeError::ConflictingArgument`].
    pub raw_args: Vec<String>,
    /// Root for the new temporary working directory.
    pub tmpdir: Option<PathBuf>,
    /// Persistent base data directory that remains after `Drop`.
    pub staticdir: Option<PathBuf>,
    /// Maximum number of process start attempts.
    pub max_retries: u8,
}

impl Default for FlorestaDConf {
    fn default() -> Self {
        Self {
            args: NodeArgs {
                network: Network::Regtest,
                v2_transport: true,
                cbf_index: true,
                prune: PruneMode::Disabled,
                txindex: false,
            },
            florestad_args: FlorestaDArgs {
                dns_seeds: false,
                allow_v1_fallback: false,
                assume_utreexo: false,
                backfill: false,
                wallet_descriptors: Vec::new(),
            },
            raw_args: Vec::new(),
            tmpdir: None,
            staticdir: None,
            max_retries: SPAWN_ATTEMPTS,
        }
    }
}

impl AsRef<NodeArgs> for FlorestaDConf {
    fn as_ref(&self) -> &NodeArgs {
        &self.args
    }
}

/// A running `florestad` [`Node`].
///
/// Floresta v0.9.1 does not accept inbound P2P connections.
/// Its JSON-RPC interface does not supply block generation or compact filter progress.
/// Unsupported commands return [`NodeError::UnsupportedCommand`].
/// A request for its P2P listener causes a panic because [`Node::get_p2p_socket`] cannot return an
/// error.
#[derive(Debug)]
pub struct FlorestaD {
    /// Handle for the `florestad` child process.
    process: Child,
    /// Unauthenticated JSON-RPC client connected to Floresta.
    pub client: Client,
    /// Plaintext Electrum client connected to the embedded Floresta server.
    pub electrum_client: RawClient<ElectrumPlaintextStream>,
    /// Base data directory and its cleanup state.
    working_directory: DataDir,
    /// Complete configuration used to start the [`Node`].
    config: FlorestaDConf,
    /// Address of the JSON-RPC listener.
    rpc_socket: SocketAddr,
    /// Address of the Electrum listener.
    electrum_socket: SocketAddr,
}

#[rustfmt::skip]
impl Node for FlorestaD {
    type Config = FlorestaDConf;

    fn get_name() -> &'static str { versions::FLORESTAD_NAME }

    fn get_bin_name() -> &'static str { versions::FLORESTAD_BIN_NAME }

    fn get_config(&self) -> &FlorestaDConf { self.get_config() }

    fn get_working_directory(&self) -> PathBuf { self.get_working_directory() }

    fn get_rpc_socket(&self) -> SocketAddr { self.get_rpc_socket() }

    fn generate(&self, _count: u32) -> Result<Vec<BlockHash>, Error> {
        Err(NodeError::UnsupportedCommand {
            node: Self::get_name(),
            command: "generate",
        }
        .into())
    }

    fn get_chain_tip(&self) -> Result<u32, Error> { self.get_chain_tip() }

    fn get_filter_tip(&self) -> Result<u32, Error> {
        Err(NodeError::UnsupportedCommand {
            node: Self::get_name(),
            command: "get_filter_tip",
        }
        .into())
    }

    fn get_block_hash(&self, height: u32) -> Result<BlockHash, Error> { self.get_block_hash(height) }

    fn call(&self, method: &str, args: &[serde_json::Value]) -> Result<serde_json::Value, Error> {
        Ok(self.client.call(method, args).map_err(NodeError::JsonRpc)?)
    }

    fn get_p2p_socket(&self) -> SocketAddr {
        panic!("florestad v0.9.1 does not accept inbound P2P connections")
    }

    fn has_peer(&self, socket: SocketAddr) -> Result<bool, Error> { self.has_peer(socket) }

    fn add_peer(&self, socket: SocketAddr) -> Result<(), Error> { self.add_peer(socket) }

    fn get_peer_count(&self) -> Result<u32, Error> { self.get_peer_count() }
}

impl FlorestaD {
    /// Start a [`FlorestaD`] using the bundled binary and default configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the function cannot find the binary or start the daemon.
    pub fn new() -> Result<Self, Error> {
        Self::from_bin(get_florestad_path()?)
    }

    /// Start a [`FlorestaD`] using the bundled binary and `conf`.
    ///
    /// # Errors
    ///
    /// Returns an error if the function cannot find the binary or start the daemon.
    /// Returns an error if the configuration is not valid.
    pub fn new_with_conf(conf: &FlorestaDConf) -> Result<Self, Error> {
        Self::from_bin_with_conf(get_florestad_path()?, conf)
    }

    /// Start the `florestad` binary at `florestad_bin` with default configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the binary path is not valid or the function cannot start the daemon.
    pub fn from_bin<P: AsRef<Path>>(florestad_bin: P) -> Result<Self, Error> {
        Self::from_bin_with_conf(florestad_bin, &FlorestaDConf::default())
    }

    /// Start the `florestad` binary at `florestad_bin` with `conf`.
    ///
    /// Each attempt uses new temporary JSON-RPC and Electrum ports.
    /// Floresta starts its Electrum server when only the [`Node`] interface is necessary.
    /// Thus, `halfin` also controls the Electrum port.
    ///
    /// # Errors
    ///
    /// Returns an error if the binary path or configuration is not valid.
    /// Returns an error if directory creation or all start attempts fail.
    #[allow(clippy::too_many_lines)]
    pub fn from_bin_with_conf<P: AsRef<Path>>(
        florestad_bin: P,
        conf: &FlorestaDConf,
    ) -> Result<Self, Error> {
        let configured_args = Self::configured_args(conf)?;
        let florestad_bin = florestad_bin.as_ref();

        if !florestad_bin.is_absolute() {
            return Err(Error::BinaryPathNotAbsolute {
                bin_name: Self::get_bin_name().to_string(),
                path: florestad_bin.display().to_string(),
            });
        }
        if !florestad_bin.is_file() {
            return Err(Error::BinaryPathNotFile {
                bin_name: Self::get_bin_name().to_string(),
                path: florestad_bin.display().to_string(),
            });
        }

        for _attempt in 0..conf.max_retries {
            let working_directory = init_data_dir(
                conf.tmpdir.as_deref(),
                conf.staticdir.as_deref(),
                "halfin-florestad-",
            )?;

            let rpc_port = get_available_port();
            let rpc_socket = SocketAddr::V4(SocketAddrV4::new(IPV4_LOCALHOST, rpc_port));
            let rpc_url = format!("http://{rpc_socket}");

            let mut electrum_port = get_available_port();
            while electrum_port == rpc_port {
                electrum_port = get_available_port();
            }
            let electrum_socket = SocketAddr::V4(SocketAddrV4::new(IPV4_LOCALHOST, electrum_port));

            let data_dir_arg = format!("--data-dir={}", working_directory.path().display());
            let rpc_address_arg = format!("--rpc-address={rpc_socket}");
            let electrum_address_arg = format!("--electrum-address={electrum_socket}");

            debug!(
                "Spawning {} [RPC_SOCKET={}, ELECTRUM_SOCKET={}, DATADIR={}]",
                Self::get_name(),
                rpc_socket,
                electrum_socket,
                working_directory.path().display()
            );

            let mut process = Command::new(florestad_bin)
                .args(&configured_args)
                .args(&conf.raw_args)
                .arg(&data_dir_arg)
                .arg(&rpc_address_arg)
                .arg(&electrum_address_arg)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(Error::FailedToSpawn)?;

            sleep(SPAWN_INTERVAL);

            match process.try_wait() {
                Ok(Some(status)) => {
                    let output = process.wait_with_output().map_err(Error::Io)?;
                    eprintln!(
                        "{} exited immediately with status={status}; stdout={}; stderr={}",
                        Self::get_name(),
                        String::from_utf8_lossy(&output.stdout).trim(),
                        String::from_utf8_lossy(&output.stderr).trim(),
                    );
                    continue;
                }
                Err(err) => {
                    debug!(
                        "{} status check failed, retrying with fresh ports: {}",
                        Self::get_name(),
                        err
                    );
                    let _ = process.kill();
                    let _ = process.wait();
                    continue;
                }
                Ok(None) => {}
            }

            if let Some(stdout) = process.stdout.take() {
                pipe_to_tracing(stdout, "florestad");
            }
            if let Some(stderr) = process.stderr.take() {
                pipe_to_tracing(stderr, "florestad");
            }

            if let Ok(client) = Self::wait_for_rpc_client(&rpc_url, Duration::from_secs(10)) {
                if let Ok(electrum_client) = Self::wait_for_electrum_client(
                    electrum_socket,
                    &mut process,
                    Duration::from_secs(10),
                ) {
                    debug!(
                        "Started {} [PID={}, RPC_SOCKET={}, ELECTRUM_SOCKET={}, DATADIR={}]",
                        Self::get_name(),
                        process.id(),
                        rpc_socket,
                        electrum_socket,
                        working_directory.path().display()
                    );

                    return Ok(Self {
                        process,
                        client,
                        electrum_client,
                        working_directory,
                        config: conf.clone(),
                        rpc_socket,
                        electrum_socket,
                    });
                }
            }

            let _ = process.kill();
            let _ = process.wait();
        }

        Err(Error::StartupAttemptsExhausted(conf.max_retries))
    }

    /// Stop Floresta through JSON-RPC and wait for it to exit.
    ///
    /// # Errors
    ///
    /// Returns an error if the stop RPC or process wait fails.
    pub fn stop(&mut self) -> Result<ExitStatus, Error> {
        debug!("Stopping {} [PID={}]", Self::get_name(), self.process.id());

        let _ = self
            .client
            .call::<serde_json::Value>("stop", &[])
            .map_err(NodeError::FailedToStop)?;
        self.process.wait().map_err(Error::Io)
    }

    /// Return the running `florestad` process ID.
    pub fn get_pid(&self) -> u32 {
        self.process.id()
    }

    /// Return the Floresta data directory.
    pub fn get_working_directory(&self) -> PathBuf {
        self.working_directory.path()
    }

    /// Return the complete configuration used to start this daemon.
    pub fn get_config(&self) -> &FlorestaDConf {
        &self.config
    }

    /// Return a reference to the Floresta JSON-RPC client.
    pub fn get_rpc_client(&self) -> &Client {
        &self.client
    }

    /// Return the Floresta JSON-RPC listener address.
    pub fn get_rpc_socket(&self) -> SocketAddr {
        self.rpc_socket
    }

    /// Return a reference to the Floresta Electrum client.
    pub fn get_electrum_client(&self) -> &RawClient<ElectrumPlaintextStream> {
        &self.electrum_client
    }

    /// Return the Floresta Electrum listener address.
    pub fn get_electrum_socket(&self) -> SocketAddr {
        self.electrum_socket
    }

    /// Return the Floresta Electrum server URL.
    pub fn get_electrum_url(&self) -> String {
        self.electrum_socket.to_string()
    }

    /// Return the current Floresta chain height.
    ///
    /// # Errors
    ///
    /// Returns an error if `getblockcount` fails or returns a non-numeric value.
    pub fn get_chain_tip(&self) -> Result<u32, Error> {
        let height = self
            .client
            .call::<serde_json::Value>("getblockcount", &[])
            .map_err(NodeError::JsonRpc)?
            .as_u64()
            .ok_or_else(|| {
                Error::UnexpectedResponse("getblockcount returned a non-numeric value".to_string())
            })? as u32;

        debug!("{}: got chain tip at height={height}", Self::get_name());
        Ok(height)
    }

    /// Return the block hash at `height`.
    ///
    /// # Errors
    ///
    /// Returns an error if `getblockhash` fails or returns an invalid hash.
    pub fn get_block_hash(&self, height: u32) -> Result<BlockHash, Error> {
        let hash = self
            .client
            .call::<serde_json::Value>("getblockhash", &[height.into()])
            .map_err(NodeError::JsonRpc)?
            .as_str()
            .ok_or_else(|| {
                Error::UnexpectedResponse("getblockhash returned a non-string value".to_string())
            })?
            .parse::<BlockHash>()
            .map_err(|err| Error::UnexpectedResponse(err.to_string()))?;

        debug!(
            "{}: got block hash at height={} hash={}",
            Self::get_name(),
            height,
            hash
        );
        Ok(hash)
    }

    /// Check whether Floresta has an outbound connection to `socket`.
    ///
    /// # Errors
    ///
    /// Returns an error if `getpeerinfo` fails or returns a non-array value.
    pub fn has_peer(&self, socket: SocketAddr) -> Result<bool, Error> {
        let peers = self
            .client
            .call::<serde_json::Value>("getpeerinfo", &[])
            .map_err(NodeError::JsonRpc)?;
        let peers = peers.as_array().ok_or_else(|| {
            Error::UnexpectedResponse("getpeerinfo returned a non-array value".to_string())
        })?;
        let has_peer = peers.iter().any(|peer| {
            peer["address"]
                .as_str()
                .and_then(|address| address.parse::<SocketAddr>().ok())
                == Some(socket)
        });

        debug!(
            "{}: checked peer connection at socket={} connected={}",
            Self::get_name(),
            socket,
            has_peer
        );
        Ok(has_peer)
    }

    /// Add `socket` as an outbound Floresta peer and wait for the connection.
    ///
    /// # Errors
    ///
    /// Returns an error if `addnode` fails or the peer does not connect before
    /// [`CONNECTION_TIMEOUT`].
    pub fn add_peer(&self, socket: SocketAddr) -> Result<(), Error> {
        self.client
            .call::<serde_json::Value>(
                "addnode",
                &[
                    socket.to_string().into(),
                    "add".into(),
                    self.config.args.v2_transport.into(),
                ],
            )
            .map_err(NodeError::JsonRpc)?;

        let mut delay = CONNECTION_INTERVAL;
        let start = Instant::now();
        while start.elapsed() < CONNECTION_TIMEOUT {
            if self.has_peer(socket)? {
                return Ok(());
            }
            sleep(delay);
            delay = (delay * 2).min(Duration::from_secs(1));
        }

        Err(NodeError::ConnectionTimeout(CONNECTION_TIMEOUT).into())
    }

    /// Return the outbound Floresta peer count.
    ///
    /// # Errors
    ///
    /// Returns an error if `getpeerinfo` fails or returns a non-array value.
    pub fn get_peer_count(&self) -> Result<u32, Error> {
        let peers = self
            .client
            .call::<serde_json::Value>("getpeerinfo", &[])
            .map_err(NodeError::JsonRpc)?;
        let count = peers
            .as_array()
            .ok_or_else(|| {
                Error::UnexpectedResponse("getpeerinfo returned a non-array value".to_string())
            })?
            .len() as u32;
        Ok(count)
    }

    /// Validate typed and raw Floresta configuration.
    fn validate_configuration(conf: &FlorestaDConf) -> Result<(), Error> {
        const OPTIONS: &[&str] = &[
            "allow-v1-fallback",
            "assume-utreexo",
            "backfill",
            "cfilters",
            "daemon",
            "data-dir",
            "disable-dns-seeds",
            "electrum-address",
            "electrum-address-tls",
            "enable-electrum-tls",
            "n",
            "network",
            "pid-file",
            "rpc-address",
            "txindex",
            "wallet-descriptor",
        ];
        const BOOLEAN_OPTIONS: &[&str] = &[
            "assume-utreexo",
            "backfill",
            "cfilters",
            "daemon",
            "enable-electrum-tls",
            "txindex",
        ];

        if conf.args.prune != PruneMode::Disabled {
            return Err(NodeError::InvalidConfiguration(
                "FlorestaD does not expose configurable block pruning".to_string(),
            )
            .into());
        }
        if conf.args.txindex {
            return Err(NodeError::InvalidConfiguration(
                "FlorestaD does not support a full transaction index".to_string(),
            )
            .into());
        }
        if let Some(arg) = find_conflicting_argument(&conf.raw_args, OPTIONS, BOOLEAN_OPTIONS) {
            return Err(NodeError::ConflictingArgument(arg).into());
        }

        Ok(())
    }

    /// Render Floresta CLI arguments owned by typed configuration.
    fn configured_args(conf: &FlorestaDConf) -> Result<Vec<String>, Error> {
        Self::validate_configuration(conf)?;

        let mut args = vec![format!("--network={}", conf.args.network)];
        if !conf.args.cbf_index {
            args.push("--no-cfilters".to_string());
        }
        if conf.florestad_args.allow_v1_fallback {
            args.push("--allow-v1-fallback".to_string());
        }
        if !conf.florestad_args.dns_seeds {
            args.push("--disable-dns-seeds".to_string());
        }
        if !conf.florestad_args.assume_utreexo {
            args.push("--no-assume-utreexo".to_string());
        }
        if !conf.florestad_args.backfill {
            args.push("--no-backfill".to_string());
        }
        args.extend(
            conf.florestad_args
                .wallet_descriptors
                .iter()
                .map(|descriptor| format!("--wallet-descriptor={descriptor}")),
        );

        Ok(args)
    }

    /// Wait until Floresta answers `getblockchaininfo`.
    fn wait_for_rpc_client(rpc_url: &str, timeout: Duration) -> Result<Client, Error> {
        let start = Instant::now();
        while start.elapsed() < timeout {
            let client = Client::new(rpc_url);
            if client
                .call::<serde_json::Value>("getblockchaininfo", &[])
                .is_ok()
            {
                return Ok(client);
            }
            sleep(Duration::from_millis(200));
        }

        Err(Error::ClientSetupTimeout)
    }

    /// Wait until the Floresta Electrum server answers `server.ping`.
    fn wait_for_electrum_client(
        electrum_socket: SocketAddr,
        process: &mut Child,
        timeout: Duration,
    ) -> Result<RawClient<ElectrumPlaintextStream>, Error> {
        let start = Instant::now();
        let mut last_error = None;
        while start.elapsed() < timeout {
            match process.try_wait() {
                Ok(Some(_)) | Err(_) => return Err(Error::ClientSetupTimeout),
                Ok(None) => {}
            }

            match RawClient::new(electrum_socket, Some(Duration::from_millis(500)), None) {
                Ok(client) => match client.ping() {
                    Ok(()) => return Ok(client),
                    Err(err) => last_error = Some(err),
                },
                Err(err) => last_error = Some(err),
            }
            sleep(Duration::from_millis(200));
        }

        Err(last_error.map_or(Error::ClientSetupTimeout, |source| {
            NodeError::UnresponsiveNode {
                node: Self::get_name(),
                source: NodeClientError::from(source),
            }
            .into()
        }))
    }
}

impl Drop for FlorestaD {
    fn drop(&mut self) {
        debug!(
            "{}: killing process with pid={}",
            Self::get_name(),
            self.process.id()
        );
        let _ = self.process.kill();
        let _ = self.process.wait();

        // Keep the owner alive until after the process releases its files.
        let _ = &self.working_directory;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_configuration_is_isolated_regtest() {
        let conf = FlorestaDConf::default();

        assert_eq!(conf.args.network, Network::Regtest);
        assert!(conf.args.v2_transport);
        assert!(conf.args.cbf_index);
        assert_eq!(conf.args.prune, PruneMode::Disabled);
        assert!(!conf.args.txindex);
        assert!(!conf.florestad_args.dns_seeds);
        assert!(!conf.florestad_args.allow_v1_fallback);
        assert!(!conf.florestad_args.assume_utreexo);
        assert!(!conf.florestad_args.backfill);
        assert!(conf.florestad_args.wallet_descriptors.is_empty());
        assert_eq!(
            FlorestaD::configured_args(&conf).unwrap(),
            [
                "--network=regtest",
                "--disable-dns-seeds",
                "--no-assume-utreexo",
                "--no-backfill",
            ]
        );
    }

    #[test]
    fn renders_supported_flags() {
        const PUBLIC_KEY: &str =
            "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";

        let mut conf = FlorestaDConf::default();
        conf.args.network = Network::Testnet4;
        conf.args.v2_transport = false;
        conf.args.cbf_index = false;
        conf.florestad_args.dns_seeds = true;
        conf.florestad_args.allow_v1_fallback = true;
        conf.florestad_args.assume_utreexo = true;
        conf.florestad_args.backfill = true;
        conf.florestad_args.wallet_descriptors = [
            format!("wpkh({PUBLIC_KEY})"),
            format!("sh(wpkh({PUBLIC_KEY}))"),
        ]
        .map(|descriptor| descriptor.parse().unwrap())
        .to_vec();

        assert_eq!(
            FlorestaD::configured_args(&conf).unwrap(),
            [
                "--network=testnet4".to_string(),
                "--no-cfilters".to_string(),
                "--allow-v1-fallback".to_string(),
                format!(
                    "--wallet-descriptor={}",
                    conf.florestad_args.wallet_descriptors[0]
                ),
                format!(
                    "--wallet-descriptor={}",
                    conf.florestad_args.wallet_descriptors[1]
                ),
            ]
        );
    }

    #[test]
    fn renders_v1_fallback_independently_from_manual_peer_transport() {
        let mut conf = FlorestaDConf::default();
        conf.args.v2_transport = false;

        assert!(
            !FlorestaD::configured_args(&conf)
                .unwrap()
                .contains(&"--allow-v1-fallback".to_string())
        );

        conf.args.v2_transport = true;
        conf.florestad_args.allow_v1_fallback = true;

        assert!(
            FlorestaD::configured_args(&conf)
                .unwrap()
                .contains(&"--allow-v1-fallback".to_string())
        );
    }

    #[test]
    fn rejects_unsupported_typed_configuration() {
        let mut conf = FlorestaDConf::default();
        conf.args.prune = PruneMode::Automatic(550);
        assert!(matches!(
            FlorestaD::configured_args(&conf),
            Err(Error::Node(NodeError::InvalidConfiguration(_)))
        ));

        conf.args.prune = PruneMode::Disabled;
        conf.args.txindex = true;
        assert!(matches!(
            FlorestaD::configured_args(&conf),
            Err(Error::Node(NodeError::InvalidConfiguration(_)))
        ));
    }

    #[test]
    fn rejects_owned_raw_arguments() {
        for arg in [
            "--network=bitcoin",
            "-n=signet",
            "-nregtest",
            "--data-dir=/tmp/floresta",
            "--rpc-address=127.0.0.1:8332",
            "--electrum-address=127.0.0.1:50001",
            "--no-cfilters",
            "--disable-dns-seeds",
            "--no-assume-utreexo",
            "--no-backfill",
            "--wallet-descriptor=raw(51)",
            "--allow-v1-fallback",
            "--daemon",
        ] {
            let conf = FlorestaDConf {
                raw_args: vec![arg.to_string()],
                ..FlorestaDConf::default()
            };
            assert!(matches!(
                FlorestaD::configured_args(&conf),
                Err(Error::Node(NodeError::ConflictingArgument(conflict))) if conflict == arg
            ));
        }
    }
}
