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
    // INI files can't represent lists, so store the addresses as a comma-separated string
    #[cfg_attr(target_os = "linux", serde(with = "comma_separated_addresses"))]
    pub addresses: Vec<IpAddr>,
    pub secret: String,
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
}

pub fn parse_addresses(string: &str) -> Result<Vec<IpAddr>, AddrParseError> {
    string.split(',').map(|ip| ip.trim().parse()).collect()
}

pub fn format_addresses(addresses: &[IpAddr]) -> String {
    addresses
        .iter()
        .map(|ip| ip.to_string())
        .collect::<Vec<String>>()
        .join(",")
}

#[cfg(target_os = "linux")]
mod comma_separated_addresses {
    use serde::{de::Error, Deserialize, Deserializer, Serializer};
    use std::net::IpAddr;

    pub fn serialize<S: Serializer>(
        addresses: &[IpAddr],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&super::format_addresses(addresses))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<IpAddr>, D::Error> {
        let string = String::deserialize(deserializer)?;
        super::parse_addresses(&string).map_err(D::Error::custom)
    }
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

        let username = whoami::username();

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
            whoami::username(),
            configuration_file_path
        );

        configuration.save()
    }
}

#[cfg(target_os = "linux")]
impl AppConfiguration {
    pub fn fetch() -> Result<AppConfiguration, ConfigurationError> {
        extern crate serde_ini;

        let path = Self::configuration_file_path();

        let string = std::fs::read_to_string(path)
            .map_err(|error| ConfigurationError::InvalidConfigurationFile { source: error })?;

        Self::from_ini(&string)
    }

    pub fn save(&self) -> Result<(), ConfigurationError> {
        let string = self.to_ini()?;

        let path = PathBuf::from(Self::configuration_file_path());
        std::fs::write(&path, string).map_err(|error| {
            ConfigurationError::ConfigurationFileUnwritable {
                source: error,
                path: path.into_os_string().into_string().unwrap(),
            }
        })
    }

    fn from_ini(string: &str) -> Result<AppConfiguration, ConfigurationError> {
        serde_ini::from_str(string).map_err(ConfigurationError::CorruptIniConfigurationFile)
    }

    fn to_ini(&self) -> Result<String, ConfigurationError> {
        serde_ini::to_string(self).map_err(|_e| ConfigurationError::InvalidConfiguration)
    }

    fn configuration_storage_path() -> String {
        Path::new("/etc").to_str().unwrap().to_string()
    }

    fn configuration_file_path() -> String {
        PathBuf::from(Self::configuration_storage_path())
            .join("ShutDownOnLan")
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

        let registry = Registry::with_default_root_key()?;
        let ips_string = registry.read_string(ConfigurationRegistryKeys::IpAddress)?;

        Ok(AppConfiguration {
            port_number: registry.read_u16(ConfigurationRegistryKeys::Port)?,
            addresses: parse_addresses(&ips_string).map_err(|_error| {
                ConfigurationError::RegistryKeyNotReadable(ConfigurationRegistryKeys::IpAddress)
            })?,
            secret: registry.read_string(ConfigurationRegistryKeys::Secret)?,
        })
    }

    pub fn save(&self) -> Result<(), ConfigurationError> {
        let registry = Registry::with_default_root_key()?;

        let joined_addresses = format_addresses(&self.addresses);
        registry.write_string(ConfigurationRegistryKeys::IpAddress, &joined_addresses)?;
        log::debug!("Set IP Addresses to {}", &joined_addresses);

        let u32_port_number = self.port_number as u32;
        registry.write_u32(ConfigurationRegistryKeys::Port, u32_port_number)?;
        log::debug!("Set Port to {}", u32_port_number);

        registry.write_string(ConfigurationRegistryKeys::Secret, &self.secret)?;
        log::debug!("Set secret");

        Ok(())
    }

    pub fn create_configuration_storage_if_not_exists() -> Result<(), ConfigurationError> {
        Registry::with_default_root_key()?;
        Ok(())
    }

    /// Writes defaults for any values that are missing from the registry. Existing values are never
    /// overwritten – if one of them can't be read, that's reported by `fetch` instead.
    pub fn create_configuration_if_not_exists() -> Result<(), ConfigurationError> {
        log::info!("Checking whether configuration needs to be created");

        let registry = Registry::with_default_root_key()?;
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

        Ok(())
    }
}

impl Default for AppConfiguration {
    fn default() -> Self {
        AppConfiguration {
            port_number: 53632,
            addresses: [IpAddr::from(Ipv4Addr::new(127, 0, 0, 1))].to_vec(),
            secret: "Super Secret String".to_string(),
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
}

#[cfg(windows)]
impl ConfigurationRegistryKeys {
    fn as_str(&self) -> &'static str {
        match self {
            ConfigurationRegistryKeys::IpAddress => "ip_addresses",
            ConfigurationRegistryKeys::Port => "port",
            ConfigurationRegistryKeys::Secret => "secret",
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
        Registry::with_root_key(PathBuf::from("SOFTWARE").join("ShutdownOnLan"))
    }

    fn with_root_key(path: PathBuf) -> Result<Registry, ConfigurationError> {
        let (key, disposition) = RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE)
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

        // `fs::write` truncates the file – otherwise a shorter configuration would leave the tail of
        // the previous one behind, corrupting the file.
        std::fs::write(path, bytes).map_err(|error| {
            ConfigurationError::ConfigurationFileUnwritable {
                source: error,
                path: path.display().to_string(),
            }
        })
    }
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
    CorruptIniConfigurationFile(#[source] serde_ini::de::Error),

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

    #[cfg(target_os = "linux")]
    #[test]
    fn test_ini_round_trip() {
        let mut configuration = AppConfiguration::default();
        configuration.set_addresses("10.0.1.100,::1").unwrap();

        let ini = configuration.to_ini().unwrap();
        assert!(ini.contains("addresses=10.0.1.100,::1"));
        assert_eq!(AppConfiguration::from_ini(&ini).unwrap(), configuration);
    }
}
