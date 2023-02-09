extern crate exitcode;

#[cfg(target_os = "macos")]
extern crate plist;

use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr};
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::Path;
use std::path::PathBuf;
use std::vec;
use thiserror::Error;

#[cfg(not(windows))]
use std::io::{prelude::*};

#[cfg(windows)]
use winreg::RegKey;

#[cfg(windows)]
use winreg::enums::RegDisposition;

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct AppConfiguration {
    pub port_number: u16,
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
    pub fn validate() -> Result<(), ConfigurationError> {
        Self::create_configuration_if_not_exists()?;
        AppConfiguration::fetch()?;

        Ok(())
    }

    pub fn set_address(&mut self, string: String) {
        self.set_addresses(string)
    }

    pub fn set_addresses(&mut self, string: String) {
        let ips_list: Vec<&str> = string.split(',').collect();

        self.addresses = ips_list.iter().filter_map(|&ip| ip.parse().ok()).collect();
    }

    pub fn set_port_number(&mut self, port: u16) {
        self.port_number = port
    }

    pub fn set_secret(&mut self, secret: String) {
        self.secret = secret
    }
}

#[cfg(target_os = "macos")]
impl AppConfiguration {
    pub fn fetch() -> Result<AppConfiguration, ConfigurationError> {
        log::debug!("Fetching App Configuration");
        Self::create_configuration_if_not_exists()?;

        let path = PathBuf::from(Self::configuration_file_path());
        Plist::read_configuration(&path)
    }

    pub fn save(&self) -> Result<(), ConfigurationError> {
        let path = PathBuf::from(Self::configuration_file_path());
        log::debug!("Writing configuration to {:?}", path);
        Plist::write_configuration(self, path)
    }

    pub fn delete(&self) -> Result<(), ConfigurationError> {
        let file_path = Self::configuration_file_path();
        let path = Path::new(&file_path);

        if path.exists() && path.is_file() {
            std::fs::remove_file(&file_path)?;
        }

        Ok(())
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

        let config: AppConfiguration =
            serde_ini::from_str(&string).expect("Invalid app configuration");

        Ok(config)
    }

    pub fn save(&self) -> Result<(), ConfigurationError> {
        extern crate serde_ini;
        let string = serde_ini::to_string(self).map_err(|err| ConfigurationError::InvalidConfiguration)?;

        let path = PathBuf::from(Self::configuration_file_path());
        std::fs::write(&path, string).map_err(|error| {
            ConfigurationError::ConfigurationStorageUnwritable {
                source: error,
                path: path.into_os_string().into_string().unwrap()
            }
        })
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
        let ips_list: Vec<&str> = ips_string.split(',').collect();

        let ip_addresses: Vec<IpAddr> = ips_list.iter().filter_map(|&ip| ip.parse().ok()).collect();

        Ok(AppConfiguration {
            port_number: registry.read_u16(ConfigurationRegistryKeys::Port)?,
            addresses: ip_addresses,
            secret: registry.read_string(ConfigurationRegistryKeys::Secret)?,
        })
    }

    pub fn save(self) -> Result<(), ConfigurationError> {
        let registry = Registry::with_default_root_key()?;

        let addresses: Vec<String> = self.addresses.iter().map(|ip| ip.to_string()).collect();

        let joined_addresses = addresses.join(",");
        registry.write_string(ConfigurationRegistryKeys::IpAddress, &joined_addresses)?;
        log::debug!("Set IP Addresses to {}", &joined_addresses);

        let u32_port_number = self.port_number as u32;
        registry.write_u32(ConfigurationRegistryKeys::Port, u32_port_number)?;
        log::debug!("Set Port to {}", u32_port_number);

        registry.write_string(ConfigurationRegistryKeys::Secret, &self.secret)?;
        log::debug!("Set secret to {}", self.secret);

        Ok(())
    }

    pub fn create_configuration_storage_if_not_exists() -> Result<(), ConfigurationError> {
        Registry::with_default_root_key()?;
        Ok(())
    }

    pub fn create_configuration_if_not_exists() -> Result<(), ConfigurationError> {
        log::info!("Checking whether configuration needs to be created");

        if let Ok(existing_configuration) = Self::fetch() {
            log::info!("Found existing configuration – skipping creation");
            log::info!("Configuration: {:?}", existing_configuration);
            return Ok(())
        }

        log::info!("Writing default configuration to registry");
        let configuration = AppConfiguration::default();
        configuration.save()?;
        log::info!("Default configuration written to registry");

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

impl ToSocketAddrs for AppConfiguration {
    type Iter = vec::IntoIter<SocketAddr>;

    fn to_socket_addrs(&self) -> std::io::Result<vec::IntoIter<SocketAddr>> {
        let mut addresses: Vec<SocketAddr> = Vec::new();

        log::info!("Read configuration with port number: {:?}", self.port_number);

        let address = IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0));

        addresses.push(SocketAddr::from((address, self.port_number)));

        let ret = addresses.into_iter();
        Ok(ret)
    }
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy)]
enum ConfigurationRegistryKeys {
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
        Registry::with_root_key(Path::new("SOFTWARE").join("ShutdownOnLan"))
    }

    fn with_root_key(path: PathBuf) -> Result<Registry, ConfigurationError> {
        let (key, disposition) = RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE)
            .create_subkey(&path)
            .expect("Failed to read app configuration");

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

    fn read_string(&self, key: ConfigurationRegistryKeys) -> Result<String, ConfigurationError> {
        self.root_key
            .get_value(key)
            .map_err(|_error| ConfigurationError::RegistryKeyNotReadable(key))
            .map(|regval: String| regval as String)
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
            .map(|regval: u32| regval as u32)
    }

    fn write_string(
        &self,
        key: ConfigurationRegistryKeys,
        value: &String,
    ) -> Result<(), ConfigurationError> {
        match self.root_key.set_value(key, value) {
            Ok(()) => Ok(()),
            Err(error) => Err(ConfigurationError::RegistryKeyNotWritable(key)),
        }
    }

    fn write_u32(
        &self,
        key: ConfigurationRegistryKeys,
        value: u32,
    ) -> Result<(), ConfigurationError> {
        match self.root_key.set_value(key, &value) {
            Ok(()) => Ok(()),
            Err(error) => Err(ConfigurationError::RegistryKeyNotWritable(key)),
        }
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
        let file = std::fs::File::open(path)
            .map_err(|error| ConfigurationError::MissingConfigurationFile(error))?;

        let mut reader = std::io::BufReader::new(file);
        let mut bytes = Vec::new();

        // Read the file into `bytes`
        reader
            .read_to_end(&mut bytes)
            .map_err(|error| ConfigurationError::InvalidConfigurationFile { source: error })?;

        AppConfiguration::try_from(bytes)
    }

    pub fn write_configuration(
        configuration: &AppConfiguration,
        path: PathBuf,
    ) -> Result<(), ConfigurationError> {
        // `open `doesn't create the file if needed, so we need to
        if !path.exists() {
            let _ = std::fs::File::create(&path)?;
        }

        let mut file_handle = std::fs::File::options()
            .write(true)
            .open(path)
            .map_err(|_e| {
                ConfigurationError::ConfigurationFileUnwritable
            })?;

        let mut buf = std::io::BufWriter::new(Vec::new());
        plist::to_writer_xml(&mut buf, &configuration)
            .map_err(|_e| ConfigurationError::InvalidConfiguration)?;

        let bytes = buf.into_inner().unwrap();
        file_handle
            .write_all(&bytes)
            .map_err(|_e| ConfigurationError::InvalidConfiguration)?;

        Ok(())
    }
}

#[derive(Error, Debug)]
pub enum ConfigurationError {
    #[error("No Configuration File at Path")]
    MissingConfigurationFile(#[from] std::io::Error),

    #[error("Contents of Configuration File Are Invalid")]
    InvalidConfigurationFile { source: std::io::Error },

    #[cfg(target_os = "macos")]
    #[error("Contents of Configuration File Are Invalid")]
    CorruptConfigurationFile(#[from] plist::Error),

    #[error("The configuration file in memory can't be converted to an on-disk representation")]
    InvalidConfiguration,

    #[cfg(windows)]
    #[error("Unable to read to a registry key")]
    RegistryKeyNotReadable(ConfigurationRegistryKeys),

    #[cfg(windows)]
    #[error("Unable to write to a registry key")]
    RegistryKeyNotWritable(ConfigurationRegistryKeys),

    #[error("Unable to write to configuration storage directory")]
    ConfigurationStorageUnwritable {
        source: std::io::Error,
        path: String,
    },

    #[error("Unable to write to configuration file: {}", exitcode::CANTCREAT)]
    ConfigurationFileUnwritable,
    // #[error("Unable to write configuration – it is not valid")]
    // ConfigurationSerializationError,
}

// mod tests {
//     use super::*;

//     fn delete_existing_configuration_before_starting() {
//         let config = AppConfiguration::default();
//         config.save().unwrap();
//     }

//     #[test]
//     fn test_that_configuration_is_created() {
//         delete_existing_configuration_before_starting();
//         assert_eq!(
//             AppConfiguration::fetch().unwrap(),
//             AppConfiguration::default()
//         );
//     }

//     #[test]
//     fn test_that_set_ip_address_works() {
//         delete_existing_configuration_before_starting();

//         let mut default = AppConfiguration::fetch().unwrap();
//         default.set_address("10.0.1.1".to_string());
//         default.save().unwrap();

//         assert_eq!(
//             "10.0.1.1",
//             AppConfiguration::fetch()
//                 .unwrap()
//                 .addresses
//                 .first()
//                 .unwrap()
//                 .to_string()
//         );
//     }

//     #[test]
//     fn test_that_set_port_works() {
//         delete_existing_configuration_before_starting();

//         let mut default = AppConfiguration::fetch().unwrap();
//         default.set_port_number(1234);
//         default.save().unwrap();

//         assert_eq!(1234, AppConfiguration::fetch().unwrap().port_number);
//     }
// }
