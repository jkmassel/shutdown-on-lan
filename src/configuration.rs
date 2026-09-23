#[cfg(target_os = "macos")]
extern crate plist;

use serde::{Deserialize, Serialize};
use std::net::{AddrParseError, IpAddr, Ipv4Addr};
use std::net::{SocketAddr, ToSocketAddrs};
#[cfg(not(windows))]
use std::path::Path;
use std::path::PathBuf;
use std::vec;
use thiserror::Error;

#[cfg(windows)]
use winreg::RegKey;

#[cfg(windows)]
use winreg::enums::RegDisposition;

/// The longest secret we'll accept, in bytes.
pub const MAX_SECRET_LENGTH: usize = 4096;

#[derive(Serialize, Deserialize, PartialEq, Eq)]
pub struct AppConfiguration {
    pub port_number: u16,
    pub addresses: Vec<IpAddr>,
    pub secret: String,
    /// The client addresses allowed to connect. Empty means any client may connect.
    // Missing from configurations written by older versions, so default to allowing any client
    #[serde(default)]
    pub allowed_sources: Vec<IpAddr>,
}

pub trait AppConfigurationStorage {
    fn fetch() -> Result<AppConfiguration, ConfigurationError>;
    fn save(&self) -> Result<(), ConfigurationError>;
    fn delete(&self) -> Result<(), ConfigurationError>;

    fn configuration_storage_path() -> String;
    fn configuration_file_path() -> String;

    fn create_configuration_if_not_exists() -> Result<(), ConfigurationError>;
    fn create_configuration_storage_if_not_exists() -> Result<(), ConfigurationError>;
}

impl AppConfiguration {
    /// Reads the configuration, creating it from defaults first if needed.
    pub fn load() -> Result<AppConfiguration, ConfigurationError> {
        Self::create_configuration_if_not_exists()?;
        Self::fetch()
    }

    pub fn set_addresses(&mut self, string: &str) -> Result<(), AddrParseError> {
        self.addresses = parse_addresses(string)?;
        Ok(())
    }

    /// Sets the allowed client addresses from a comma-separated list. An empty string allows any client.
    pub fn set_allowed_sources(&mut self, string: &str) -> Result<(), AddrParseError> {
        self.allowed_sources = parse_optional_addresses(string)?;
        Ok(())
    }

    pub fn set_secret(&mut self, secret: String) -> Result<(), ConfigurationError> {
        if secret.is_empty() || secret.len() > MAX_SECRET_LENGTH {
            return Err(ConfigurationError::InvalidSecret);
        }

        self.secret = secret;
        Ok(())
    }

    /// Whether a connection received on the local interface `ip` is allowed to shut down the machine.
    pub fn accepts_connections_on(&self, ip: &IpAddr) -> bool {
        self.addresses.contains(ip)
    }

    /// Whether a client at `ip` is allowed to connect.
    pub fn accepts_connections_from(&self, ip: &IpAddr) -> bool {
        self.allowed_sources.is_empty() || self.allowed_sources.contains(ip)
    }
}

pub fn parse_addresses(string: &str) -> Result<Vec<IpAddr>, AddrParseError> {
    string.split(',').map(|ip| ip.trim().parse()).collect()
}

/// Like `parse_addresses`, but an empty string is an empty list rather than an error.
pub fn parse_optional_addresses(string: &str) -> Result<Vec<IpAddr>, AddrParseError> {
    if string.trim().is_empty() {
        return Ok(Vec::new());
    }

    parse_addresses(string)
}

pub fn format_addresses(addresses: &[IpAddr]) -> String {
    addresses
        .iter()
        .map(|ip| ip.to_string())
        .collect::<Vec<String>>()
        .join(",")
}

#[cfg(target_os = "macos")]
impl AppConfiguration {
    pub fn fetch() -> Result<AppConfiguration, ConfigurationError> {
        log::debug!("Fetching App Configuration");

        let path = PathBuf::from(Self::configuration_file_path());
        Plist::read_configuration(&path)
    }

    pub fn save(&self) -> Result<(), ConfigurationError> {
        let path = PathBuf::from(Self::configuration_file_path());
        log::debug!("Writing configuration to {:?}", path);
        Plist::write_configuration(self, &path)
    }

    fn configuration_storage_path() -> String {
        extern crate dirs;

        let username = whoami::username().unwrap_or_default();

        if username == "root" {
            let path = Path::new("/Library/Application Support/ShutdownOnLan").to_path_buf();
            log::info!("Detected Configuration Path: {:?}", path);
            return path.into_os_string().into_string().unwrap();
        }

        let path = dirs::home_dir()
            .expect("failed to find home directory")
            .join("Library")
            .join("Application Support")
            .join("ShutdownOnLan")
            .as_path()
            .to_owned();

        log::info!("Detected Configuration Path: {:?}", path);

        path.into_os_string().into_string().unwrap()
    }

    fn configuration_file_path() -> String {
        PathBuf::from(Self::configuration_storage_path())
            .join("ShutDownOnLan.plist")
            .into_os_string()
            .into_string()
            .unwrap()
    }

    fn create_configuration_storage_if_not_exists() -> Result<(), ConfigurationError> {
        let path = PathBuf::from(Self::configuration_storage_path());

        if path.exists() && path.is_dir() {
            return Ok(());
        }

        log::debug!("Creating configuration storage at {:?}", path);

        std::fs::create_dir(&path).map_err(|error| {
            ConfigurationError::ConfigurationStorageUnwritable {
                source: error,
                path: path.into_os_string().into_string().unwrap(),
            }
        })
    }

    fn create_configuration_if_not_exists() -> Result<(), ConfigurationError> {
        log::debug!("Checking whether configuration needs to be created");

        Self::create_configuration_storage_if_not_exists()?;

        let path = PathBuf::from(Self::configuration_file_path());

        if path.exists() {
            log::debug!("Configuration Exists");
            return Ok(());
        }

        log::info!("Creating Configuration File from Defaults");

        let configuration = AppConfiguration::default();
        let configuration_file_path = Self::configuration_file_path();

        log::debug!(
            "Creating configuration for {:?} at {:?}",
            whoami::username().unwrap_or_default(),
            configuration_file_path
        );

        configuration.save()
    }
}

#[cfg(target_os = "linux")]
impl AppConfiguration {
    pub fn fetch() -> Result<AppConfiguration, ConfigurationError> {
        let path = Self::configuration_file_path();

        let string = std::fs::read_to_string(path)
            .map_err(|error| ConfigurationError::InvalidConfigurationFile { source: error })?;

        Self::from_toml(&string)
    }

    pub fn save(&self) -> Result<(), ConfigurationError> {
        let string = self.to_toml()?;

        let path = PathBuf::from(Self::configuration_file_path());
        write_private_file(&path, string.as_bytes()).map_err(|error| {
            ConfigurationError::ConfigurationFileUnwritable {
                source: error,
                path: path.into_os_string().into_string().unwrap(),
            }
        })
    }

    fn from_toml(string: &str) -> Result<AppConfiguration, ConfigurationError> {
        toml::from_str(string).map_err(ConfigurationError::CorruptTomlConfigurationFile)
    }

    fn to_toml(&self) -> Result<String, ConfigurationError> {
        toml::to_string(self).map_err(|_e| ConfigurationError::InvalidConfiguration)
    }

    fn configuration_storage_path() -> String {
        Path::new("/etc").to_str().unwrap().to_string()
    }

    fn configuration_file_path() -> String {
        PathBuf::from(Self::configuration_storage_path())
            .join("shutdown-on-lan.toml")
            .into_os_string()
            .to_str()
            .unwrap()
            .to_string()
    }

    fn create_configuration_storage_if_not_exists() -> Result<(), ConfigurationError> {
        let path = PathBuf::from(Self::configuration_storage_path());

        if path.exists() && path.is_dir() {
            return Ok(());
        }

        log::debug!("Creating configuration storage at {:?}", path);

        std::fs::create_dir(&path).map_err(|error| {
            ConfigurationError::ConfigurationStorageUnwritable {
                source: error,
                path: path.into_os_string().into_string().unwrap(),
            }
        })
    }

    fn create_configuration_if_not_exists() -> Result<(), ConfigurationError> {
        log::debug!("Checking whether configuration needs to be created");

        Self::create_configuration_storage_if_not_exists()?;

        let path = PathBuf::from(Self::configuration_file_path());

        if path.exists() {
            log::debug!("Configuration Exists");
            return Ok(());
        }

        log::info!("Creating Configuration File from Defaults");

        let configuration = AppConfiguration::default();

        log::debug!("Creating configuration at {:?}", path);

        configuration.save()
    }
}

#[cfg(windows)]
impl AppConfiguration {
    pub fn fetch() -> Result<AppConfiguration, ConfigurationError> {
        log::info!("Looking up configuration");
        Self::fetch_from(&Registry::with_default_root_key()?)
    }

    pub fn save(&self) -> Result<(), ConfigurationError> {
        self.save_to(&Registry::with_default_root_key()?)
    }

    pub fn create_configuration_storage_if_not_exists() -> Result<(), ConfigurationError> {
        Registry::with_default_root_key()?;
        Ok(())
    }

    pub fn create_configuration_if_not_exists() -> Result<(), ConfigurationError> {
        log::info!("Checking whether configuration needs to be created");
        Self::write_missing_defaults(&Registry::with_default_root_key()?)
    }

    fn fetch_from(registry: &Registry) -> Result<AppConfiguration, ConfigurationError> {
        let ips_string = registry.read_string(ConfigurationRegistryKeys::IpAddress)?;

        Ok(AppConfiguration {
            port_number: registry.read_u16(ConfigurationRegistryKeys::Port)?,
            addresses: parse_addresses(&ips_string).map_err(|_error| {
                ConfigurationError::RegistryKeyNotReadable(ConfigurationRegistryKeys::IpAddress)
            })?,
            secret: registry.read_string(ConfigurationRegistryKeys::Secret)?,
            allowed_sources: parse_optional_addresses(
                &registry.read_string(ConfigurationRegistryKeys::AllowedSources)?,
            )
            .map_err(|_error| {
                ConfigurationError::RegistryKeyNotReadable(
                    ConfigurationRegistryKeys::AllowedSources,
                )
            })?,
        })
    }

    fn save_to(&self, registry: &Registry) -> Result<(), ConfigurationError> {
        let joined_addresses = format_addresses(&self.addresses);
        registry.write_string(ConfigurationRegistryKeys::IpAddress, &joined_addresses)?;
        log::debug!("Set IP Addresses to {}", &joined_addresses);

        let u32_port_number = self.port_number as u32;
        registry.write_u32(ConfigurationRegistryKeys::Port, u32_port_number)?;
        log::debug!("Set Port to {}", u32_port_number);

        registry.write_string(ConfigurationRegistryKeys::Secret, &self.secret)?;
        log::debug!("Set secret");

        let joined_sources = format_addresses(&self.allowed_sources);
        registry.write_string(ConfigurationRegistryKeys::AllowedSources, &joined_sources)?;
        log::debug!("Set allowed sources to {:?}", &joined_sources);

        Ok(())
    }

    /// Writes defaults for any values that are missing from the registry. Existing values are never
    /// overwritten – if one of them exists but can't be read, that's reported as an error instead.
    fn write_missing_defaults(registry: &Registry) -> Result<(), ConfigurationError> {
        let defaults = AppConfiguration::default();

        if !registry.contains::<String>(ConfigurationRegistryKeys::IpAddress)? {
            log::info!("Writing default IP addresses to registry");
            registry.write_string(
                ConfigurationRegistryKeys::IpAddress,
                &format_addresses(&defaults.addresses),
            )?;
        }

        if !registry.contains::<u32>(ConfigurationRegistryKeys::Port)? {
            log::info!("Writing default port to registry");
            registry.write_u32(ConfigurationRegistryKeys::Port, defaults.port_number as u32)?;
        }

        if !registry.contains::<String>(ConfigurationRegistryKeys::Secret)? {
            log::info!("Writing default secret to registry");
            registry.write_string(ConfigurationRegistryKeys::Secret, &defaults.secret)?;
        }

        if !registry.contains::<String>(ConfigurationRegistryKeys::AllowedSources)? {
            log::info!("Writing default allowed sources to registry");
            registry.write_string(
                ConfigurationRegistryKeys::AllowedSources,
                &format_addresses(&defaults.allowed_sources),
            )?;
        }

        Ok(())
    }
}

impl Default for AppConfiguration {
    fn default() -> Self {
        AppConfiguration {
            port_number: 53632,
            addresses: [IpAddr::from(Ipv4Addr::new(127, 0, 0, 1))].to_vec(),
            secret: "Super Secret String".to_string(),
            allowed_sources: Vec::new(),
        }
    }
}

// Implemented by hand so the secret never ends up in a log
impl std::fmt::Debug for AppConfiguration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppConfiguration")
            .field("port_number", &self.port_number)
            .field("addresses", &self.addresses)
            .field("secret", &"<redacted>")
            .field("allowed_sources", &self.allowed_sources)
            .finish()
    }
}

impl ToSocketAddrs for AppConfiguration {
    type Iter = vec::IntoIter<SocketAddr>;

    fn to_socket_addrs(&self) -> std::io::Result<vec::IntoIter<SocketAddr>> {
        let mut addresses: Vec<SocketAddr> = Vec::new();

        log::info!(
            "Read configuration with port number: {:?}",
            self.port_number
        );

        // Bind every interface rather than just the configured `addresses`. On Windows the service starts
        // before the network interfaces are up, so binding a specific address fails at boot and the service
        // never listens. Instead, `listener_service` rejects connections that arrive on interfaces that
        // aren't in `addresses` after `accept`.
        let address = IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0));

        addresses.push(SocketAddr::from((address, self.port_number)));

        let ret = addresses.into_iter();
        Ok(ret)
    }
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy)]
pub enum ConfigurationRegistryKeys {
    IpAddress,
    Port,
    Secret,
    AllowedSources,
}

#[cfg(windows)]
impl ConfigurationRegistryKeys {
    fn as_str(&self) -> &'static str {
        match self {
            ConfigurationRegistryKeys::IpAddress => "ip_addresses",
            ConfigurationRegistryKeys::Port => "port",
            ConfigurationRegistryKeys::Secret => "secret",
            ConfigurationRegistryKeys::AllowedSources => "allowed_sources",
        }
    }
}

#[cfg(windows)]
impl AsRef<std::ffi::OsStr> for ConfigurationRegistryKeys {
    fn as_ref(&self) -> &std::ffi::OsStr {
        std::ffi::OsStr::new(self.as_str())
    }
}

#[cfg(windows)]
struct Registry {
    root_key: RegKey,
}

#[cfg(windows)]
impl Registry {
    fn with_default_root_key() -> Result<Registry, ConfigurationError> {
        Registry::with_root_key(
            winreg::enums::HKEY_LOCAL_MACHINE,
            PathBuf::from("SOFTWARE").join("ShutdownOnLan"),
        )
    }

    fn with_root_key(
        predefined_key: winreg::HKEY,
        path: PathBuf,
    ) -> Result<Registry, ConfigurationError> {
        let (key, disposition) = RegKey::predef(predefined_key)
            .create_subkey(&path)
            .map_err(ConfigurationError::RegistryUnavailable)?;

        match disposition {
            RegDisposition::REG_CREATED_NEW_KEY => {
                log::info!("Created New Registry Key");
            }
            RegDisposition::REG_OPENED_EXISTING_KEY => {
                log::info!("Using Existing Registry Key");
            }
        }

        Ok(Registry { root_key: key })
    }

    /// Whether `key` has a value. Errors other than the value being absent are reported, so that a
    /// value that exists but can't be read isn't mistaken for a missing one and overwritten.
    fn contains<T: winreg::types::FromRegValue>(
        &self,
        key: ConfigurationRegistryKeys,
    ) -> Result<bool, ConfigurationError> {
        match self.root_key.get_value::<T, _>(key) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(_error) => Err(ConfigurationError::RegistryKeyNotReadable(key)),
        }
    }

    fn read_string(&self, key: ConfigurationRegistryKeys) -> Result<String, ConfigurationError> {
        self.root_key
            .get_value(key)
            .map_err(|_error| ConfigurationError::RegistryKeyNotReadable(key))
    }

    fn read_u16(&self, key: ConfigurationRegistryKeys) -> Result<u16, ConfigurationError> {
        use std::convert::TryFrom;
        let value = self.read_u32(key)?;
        u16::try_from(value).map_err(|_error| ConfigurationError::RegistryKeyNotReadable(key))
    }

    fn read_u32(&self, key: ConfigurationRegistryKeys) -> Result<u32, ConfigurationError> {
        self.root_key
            .get_value(key)
            .map_err(|_error| ConfigurationError::RegistryKeyNotReadable(key))
    }

    fn write_string(
        &self,
        key: ConfigurationRegistryKeys,
        value: &String,
    ) -> Result<(), ConfigurationError> {
        self.root_key
            .set_value(key, value)
            .map_err(|_error| ConfigurationError::RegistryKeyNotWritable(key))
    }

    fn write_u32(
        &self,
        key: ConfigurationRegistryKeys,
        value: u32,
    ) -> Result<(), ConfigurationError> {
        self.root_key
            .set_value(key, &value)
            .map_err(|_error| ConfigurationError::RegistryKeyNotWritable(key))
    }
}

#[cfg(target_os = "macos")]
use std::convert::TryFrom;

#[cfg(target_os = "macos")]
impl TryFrom<Vec<u8>> for AppConfiguration {
    type Error = ConfigurationError;

    fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
        plist::from_bytes(&value).map_err(ConfigurationError::CorruptConfigurationFile)
    }
}

#[cfg(target_os = "macos")]
struct Plist {}

#[cfg(target_os = "macos")]
impl Plist {
    pub fn read_configuration(path: &Path) -> Result<AppConfiguration, ConfigurationError> {
        let bytes = std::fs::read(path).map_err(ConfigurationError::MissingConfigurationFile)?;
        AppConfiguration::try_from(bytes)
    }

    pub fn write_configuration(
        configuration: &AppConfiguration,
        path: &Path,
    ) -> Result<(), ConfigurationError> {
        let mut bytes = Vec::new();
        plist::to_writer_xml(&mut bytes, &configuration)
            .map_err(|_e| ConfigurationError::InvalidConfiguration)?;

        write_private_file(path, &bytes).map_err(|error| {
            ConfigurationError::ConfigurationFileUnwritable {
                source: error,
                path: path.display().to_string(),
            }
        })
    }
}

/// Replaces the contents of `path` with `contents`, leaving the file readable only by its owner – it
/// holds the secret, which any local user could otherwise read.
#[cfg(unix)]
fn write_private_file(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    // Truncate the file – otherwise a shorter configuration would leave the tail of the previous one
    // behind, corrupting the file.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;

    // `mode` only applies when the file is created, so also restrict a file written by an older version
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(contents)
}

#[derive(Error, Debug)]
pub enum ConfigurationError {
    #[error("No Configuration File at Path")]
    MissingConfigurationFile(#[from] std::io::Error),

    #[cfg(target_os = "linux")]
    #[error("Contents of Configuration File Are Invalid")]
    InvalidConfigurationFile { source: std::io::Error },

    #[cfg(target_os = "macos")]
    #[error("Contents of Configuration File Are Invalid")]
    CorruptConfigurationFile(#[from] plist::Error),

    #[cfg(target_os = "linux")]
    #[error("Contents of Configuration File Are Invalid")]
    CorruptTomlConfigurationFile(#[source] toml::de::Error),

    #[error("The configuration file in memory can't be converted to an on-disk representation")]
    InvalidConfiguration,

    #[error("The secret must be between 1 and {} bytes long", MAX_SECRET_LENGTH)]
    InvalidSecret,

    #[cfg(windows)]
    #[error("Unable to open the configuration registry key")]
    RegistryUnavailable(#[source] std::io::Error),

    #[cfg(windows)]
    #[error("Unable to read registry value {0:?}")]
    RegistryKeyNotReadable(ConfigurationRegistryKeys),

    #[cfg(windows)]
    #[error("Unable to write registry value {0:?}")]
    RegistryKeyNotWritable(ConfigurationRegistryKeys),

    #[error("Unable to write to configuration storage directory")]
    ConfigurationStorageUnwritable {
        source: std::io::Error,
        path: String,
    },

    #[error("Unable to write to configuration file at {path}")]
    ConfigurationFileUnwritable {
        source: std::io::Error,
        path: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_addresses_accepts_a_comma_separated_list() {
        let mut configuration = AppConfiguration::default();
        configuration.set_addresses("10.0.1.100, ::1").unwrap();

        assert_eq!(
            configuration.addresses,
            vec![
                "10.0.1.100".parse::<IpAddr>().unwrap(),
                "::1".parse::<IpAddr>().unwrap()
            ]
        );
    }

    #[test]
    fn test_set_addresses_rejects_invalid_addresses_without_modifying_the_configuration() {
        let mut configuration = AppConfiguration::default();

        assert!(configuration
            .set_addresses("10.0.1.100,10.0.1.300")
            .is_err());
        assert!(configuration.set_addresses("").is_err());
        assert_eq!(
            configuration.addresses,
            AppConfiguration::default().addresses
        );
    }

    #[test]
    fn test_empty_allowed_sources_accepts_any_client() {
        let mut configuration = AppConfiguration::default();
        configuration.set_allowed_sources("").unwrap();

        assert!(configuration.accepts_connections_from(&"10.0.1.50".parse().unwrap()));
    }

    #[test]
    fn test_allowed_sources_only_accepts_listed_clients() {
        let mut configuration = AppConfiguration::default();
        configuration
            .set_allowed_sources("10.0.1.50, 10.0.1.51")
            .unwrap();

        assert!(configuration.accepts_connections_from(&"10.0.1.51".parse().unwrap()));
        assert!(!configuration.accepts_connections_from(&"10.0.1.52".parse().unwrap()));
        assert!(configuration.set_allowed_sources("10.0.1.300").is_err());
    }

    #[test]
    fn test_set_secret_enforces_length_limits() {
        let mut configuration = AppConfiguration::default();

        assert!(configuration.set_secret(String::new()).is_err());
        assert!(configuration
            .set_secret("a".repeat(MAX_SECRET_LENGTH + 1))
            .is_err());
        assert!(configuration
            .set_secret("a".repeat(MAX_SECRET_LENGTH))
            .is_ok());
    }

    #[test]
    fn test_default_configuration_only_accepts_connections_on_loopback() {
        let configuration = AppConfiguration::default();

        assert!(configuration.accepts_connections_on(&"127.0.0.1".parse().unwrap()));
        assert!(!configuration.accepts_connections_on(&"10.0.1.100".parse().unwrap()));
    }

    #[test]
    fn test_debug_output_does_not_include_the_secret() {
        let output = format!("{:?}", AppConfiguration::default());
        assert!(!output.contains("Super Secret String"));
    }

    #[cfg(unix)]
    #[test]
    fn test_configuration_file_is_only_readable_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;

        let path =
            std::env::temp_dir().join(format!("shutdown-on-lan-test-{}.mode", std::process::id()));
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;

        write_private_file(&path, b"first").unwrap();
        assert_eq!(mode(&path), 0o600);

        // A file left readable by an older version is restricted the next time it's written
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_private_file(&path, b"second").unwrap();
        let contents = std::fs::read(&path).unwrap();
        let final_mode = mode(&path);
        std::fs::remove_file(&path).unwrap();

        assert_eq!(final_mode, 0o600);
        assert_eq!(contents, b"second");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_writing_a_shorter_plist_replaces_the_previous_one() {
        let path =
            std::env::temp_dir().join(format!("shutdown-on-lan-test-{}.plist", std::process::id()));

        let mut configuration = AppConfiguration::default();
        configuration.set_secret("a".repeat(1024)).unwrap();
        Plist::write_configuration(&configuration, &path).unwrap();

        configuration.set_secret("short".to_string()).unwrap();
        Plist::write_configuration(&configuration, &path).unwrap();

        let result = Plist::read_configuration(&path);
        std::fs::remove_file(&path).unwrap();

        assert_eq!(result.unwrap(), configuration);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_plist_without_allowed_sources_accepts_any_client() {
        let plist = br#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>port_number</key>
	<integer>53632</integer>
	<key>addresses</key>
	<array>
		<string>127.0.0.1</string>
	</array>
	<key>secret</key>
	<string>Super Secret String</string>
</dict>
</plist>"#;

        let configuration = AppConfiguration::try_from(plist.to_vec()).unwrap();
        assert!(configuration.allowed_sources.is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_packaged_configuration_matches_the_defaults() {
        let packaged = include_str!("../build/linux/shutdown-on-lan.toml");
        assert_eq!(
            AppConfiguration::from_toml(packaged).unwrap(),
            AppConfiguration::default()
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_toml_round_trip() {
        let mut configuration = AppConfiguration::default();
        configuration.set_addresses("10.0.1.100,::1").unwrap();
        configuration.set_allowed_sources("10.0.1.50").unwrap();

        let toml = configuration.to_toml().unwrap();
        assert!(toml.contains(r#"addresses = ["10.0.1.100", "::1"]"#));
        assert!(toml.contains(r#"allowed_sources = ["10.0.1.50"]"#));
        assert_eq!(AppConfiguration::from_toml(&toml).unwrap(), configuration);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_toml_without_allowed_sources_accepts_any_client() {
        let configuration = AppConfiguration::default();

        let toml = configuration.to_toml().unwrap();
        assert_eq!(AppConfiguration::from_toml(&toml).unwrap(), configuration);

        let hand_written = r#"
            port_number = 53632
            addresses = ["127.0.0.1"]
            secret = "Super Secret String"
        "#;
        assert!(AppConfiguration::from_toml(hand_written)
            .unwrap()
            .allowed_sources
            .is_empty());
    }

    /// A registry key under `HKEY_CURRENT_USER` (so no admin rights are needed) that's deleted when
    /// the test finishes.
    #[cfg(windows)]
    struct TestRegistry {
        path: PathBuf,
        registry: Registry,
    }

    #[cfg(windows)]
    impl TestRegistry {
        fn new(name: &str) -> TestRegistry {
            let path = PathBuf::from("Software").join(format!(
                "ShutdownOnLan-Test-{}-{}",
                std::process::id(),
                name
            ));
            let registry =
                Registry::with_root_key(winreg::enums::HKEY_CURRENT_USER, path.clone()).unwrap();

            TestRegistry { path, registry }
        }

        fn custom_configuration() -> AppConfiguration {
            let mut configuration = AppConfiguration {
                port_number: 12345,
                ..AppConfiguration::default()
            };
            configuration.set_addresses("10.0.1.100").unwrap();
            configuration
                .set_secret("custom secret".to_string())
                .unwrap();
            configuration.set_allowed_sources("10.0.1.50").unwrap();
            configuration
        }
    }

    #[cfg(windows)]
    impl Drop for TestRegistry {
        fn drop(&mut self) {
            let _ = RegKey::predef(winreg::enums::HKEY_CURRENT_USER).delete_subkey_all(&self.path);
        }
    }

    #[cfg(windows)]
    #[test]
    fn test_registry_round_trip() {
        let test = TestRegistry::new("round-trip");
        let configuration = TestRegistry::custom_configuration();

        configuration.save_to(&test.registry).unwrap();

        assert_eq!(
            AppConfiguration::fetch_from(&test.registry).unwrap(),
            configuration
        );
    }

    #[cfg(windows)]
    #[test]
    fn test_empty_registry_is_filled_with_defaults() {
        let test = TestRegistry::new("empty");

        AppConfiguration::write_missing_defaults(&test.registry).unwrap();

        assert_eq!(
            AppConfiguration::fetch_from(&test.registry).unwrap(),
            AppConfiguration::default()
        );
    }

    #[cfg(windows)]
    #[test]
    fn test_upgraded_registry_gets_empty_allowed_sources() {
        let test = TestRegistry::new("upgrade");
        let configuration = TestRegistry::custom_configuration();

        // Configurations written by older versions don't have `allowed_sources`
        configuration.save_to(&test.registry).unwrap();
        test.registry
            .root_key
            .delete_value(ConfigurationRegistryKeys::AllowedSources)
            .unwrap();

        AppConfiguration::write_missing_defaults(&test.registry).unwrap();

        assert_eq!(
            test.registry
                .read_string(ConfigurationRegistryKeys::AllowedSources)
                .unwrap(),
            ""
        );

        let upgraded = AppConfiguration::fetch_from(&test.registry).unwrap();
        assert!(upgraded.allowed_sources.is_empty());
        assert!(upgraded.accepts_connections_from(&"10.0.1.99".parse().unwrap()));
        assert_eq!(upgraded.port_number, configuration.port_number);
        assert_eq!(upgraded.addresses, configuration.addresses);
        assert_eq!(upgraded.secret, configuration.secret);
    }

    #[cfg(windows)]
    #[test]
    fn test_only_missing_registry_values_are_restored() {
        let test = TestRegistry::new("missing-value");
        let configuration = TestRegistry::custom_configuration();

        configuration.save_to(&test.registry).unwrap();
        test.registry
            .root_key
            .delete_value(ConfigurationRegistryKeys::Port)
            .unwrap();

        AppConfiguration::write_missing_defaults(&test.registry).unwrap();

        assert_eq!(
            AppConfiguration::fetch_from(&test.registry).unwrap(),
            AppConfiguration {
                port_number: AppConfiguration::default().port_number,
                ..configuration
            }
        );
    }

    #[cfg(windows)]
    #[test]
    fn test_unreadable_registry_values_are_not_overwritten() {
        let test = TestRegistry::new("unreadable-value");
        let configuration = TestRegistry::custom_configuration();

        configuration.save_to(&test.registry).unwrap();

        // The port should be a DWORD – a string can't be read as one
        test.registry
            .write_string(ConfigurationRegistryKeys::Port, &"not a number".to_string())
            .unwrap();

        assert!(AppConfiguration::write_missing_defaults(&test.registry).is_err());
        assert_eq!(
            test.registry
                .read_string(ConfigurationRegistryKeys::Port)
                .unwrap(),
            "not a number"
        );
    }
}
