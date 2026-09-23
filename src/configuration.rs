use serde::{Deserialize, Serialize};
use std::net::{AddrParseError, IpAddr, Ipv4Addr};
use std::net::{SocketAddr, ToSocketAddrs};
#[cfg(not(windows))]
use std::path::Path;
#[cfg(not(target_os = "macos"))]
use std::path::PathBuf;
use std::vec;
use thiserror::Error;

#[cfg(windows)]
use winreg::RegKey;

#[cfg(windows)]
use winreg::enums::RegDisposition;

#[cfg(target_os = "macos")]
use core_foundation::{
    array::CFArray,
    base::{CFType, TCFType},
    data::CFData,
    dictionary::CFDictionary,
    number::CFNumber,
    propertylist::{CFPropertyList, CFPropertyListSubClass},
    string::{CFString, CFStringRef},
};
#[cfg(target_os = "macos")]
use std::convert::TryFrom;

/// The longest secret we'll accept, in bytes.
pub const MAX_SECRET_LENGTH: usize = 4096;

#[derive(Serialize, Deserialize, PartialEq, Eq)]
pub struct AppConfiguration {
    pub port_number: u16,
    /// The local interface addresses to accept connections on. Empty means every interface.
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

    /// Sets the local interface addresses from a comma-separated list. An empty string accepts
    /// connections on every interface.
    pub fn set_addresses(&mut self, string: &str) -> Result<(), AddrParseError> {
        self.addresses = parse_optional_addresses(string)?;
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
        self.addresses.is_empty() || self.addresses.contains(ip)
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

/// Like `format_addresses`, but describes an empty list of interface addresses as meaning every interface.
pub fn describe_addresses(addresses: &[IpAddr]) -> String {
    if addresses.is_empty() {
        "every interface".to_string()
    } else {
        format_addresses(addresses)
    }
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
        Self::fetch_from(&Preferences::system())
    }

    pub fn save(&self) -> Result<(), ConfigurationError> {
        self.save_to(&Preferences::system())
    }

    pub fn create_configuration_if_not_exists() -> Result<(), ConfigurationError> {
        log::debug!("Checking whether configuration needs to be created");
        let preferences = Preferences::system();

        let legacy_file = Path::new(LEGACY_CONFIGURATION_FILE);
        if legacy_file.exists() {
            Self::migrate_legacy_file(legacy_file, &preferences)?;
        }

        Self::write_missing_defaults(&preferences)?;

        // Only root can do this, and the service runs as root, so it fixes the permissions on startup
        if let Err(error) = preferences.restrict_permissions() {
            log::debug!("Unable to restrict the preferences file: {}", error);
        }

        Ok(())
    }

    fn fetch_from(preferences: &Preferences) -> Result<AppConfiguration, ConfigurationError> {
        Self::from_property_lists(|key| preferences.get(key))
            .map_err(ConfigurationError::PreferenceNotReadable)
    }

    /// Writes the values that differ from the stored ones. Fails without writing anything if one of them
    /// is managed by a configuration profile, because the change would have no effect.
    fn save_to(&self, preferences: &Preferences) -> Result<(), ConfigurationError> {
        let changes: Vec<(&'static str, CFPropertyList)> = self
            .to_property_lists()
            .into_iter()
            .filter(|(key, value)| preferences.get(key).as_ref() != Some(value))
            .collect();

        if let Some((key, _)) = changes.iter().find(|(key, _)| preferences.is_forced(key)) {
            return Err(ConfigurationError::PreferenceIsManaged(key));
        }

        for (key, value) in &changes {
            log::debug!("Setting {}", key);
            preferences.set(key, value);
        }

        preferences.synchronize()
    }

    /// Writes defaults for any values that are missing. Existing values (including those managed by a
    /// configuration profile) are never overwritten.
    fn write_missing_defaults(preferences: &Preferences) -> Result<(), ConfigurationError> {
        let mut changed = false;

        for (key, value) in AppConfiguration::default().to_property_lists() {
            if preferences.get(key).is_none() {
                log::info!("Writing default {} to preferences", key);
                preferences.set(key, &value);
                changed = true;
            }
        }

        if changed {
            preferences.synchronize()?;
        }

        Ok(())
    }

    /// Imports the plist written by versions before configuration moved to `CFPreferences`, then deletes
    /// it so it isn't imported again. Only the system-wide file is imported – files under users' home
    /// directories were written by running the CLI without `sudo`, and the service never read them.
    fn migrate_legacy_file(
        path: &Path,
        preferences: &Preferences,
    ) -> Result<(), ConfigurationError> {
        log::info!("Migrating configuration from {}", path.display());

        let bytes = std::fs::read(path).map_err(ConfigurationError::MissingConfigurationFile)?;
        let dictionary = core_foundation::propertylist::create_with_data(
            CFData::from_buffer(&bytes),
            core_foundation::propertylist::kCFPropertyListImmutable,
        )
        .ok()
        .and_then(|(plist, _format)| {
            unsafe { CFPropertyList::wrap_under_create_rule(plist) }.downcast_into::<CFDictionary>()
        })
        .ok_or(ConfigurationError::CorruptConfigurationFile)?;

        let legacy = Self::from_property_lists(|key| {
            dictionary
                .find(CFString::new(key).as_CFTypeRef())
                .map(|value| unsafe { CFPropertyList::wrap_under_get_rule(*value) })
        })
        .map_err(|_key| ConfigurationError::CorruptConfigurationFile)?;

        // Values managed by a configuration profile take precedence over the legacy file
        for (key, value) in legacy.to_property_lists() {
            if !preferences.is_forced(key) {
                preferences.set(key, &value);
            }
        }
        preferences.synchronize()?;

        std::fs::remove_file(path).map_err(|error| {
            ConfigurationError::ConfigurationFileUnwritable {
                source: error,
                path: path.display().to_string(),
            }
        })?;

        if let Some(directory) = path.parent() {
            // Only succeeds if nothing else was stored alongside it
            let _ = std::fs::remove_dir(directory);
        }

        log::info!("Migrated configuration to {}", PREFERENCES_FILE);
        Ok(())
    }

    /// Reads a configuration from property list values, returning the key of the first value that's
    /// missing or has the wrong type. `allowed_sources` may be missing, because older versions didn't
    /// write it.
    fn from_property_lists(
        get: impl Fn(&'static str) -> Option<CFPropertyList>,
    ) -> Result<AppConfiguration, &'static str> {
        let allowed_sources = match get(PreferenceKeys::ALLOWED_SOURCES) {
            Some(value) => {
                property_list_to_addresses(value).ok_or(PreferenceKeys::ALLOWED_SOURCES)?
            }
            None => Vec::new(),
        };

        Ok(AppConfiguration {
            port_number: get(PreferenceKeys::PORT)
                .and_then(property_list_to_u16)
                .ok_or(PreferenceKeys::PORT)?,
            addresses: get(PreferenceKeys::ADDRESSES)
                .and_then(property_list_to_addresses)
                .ok_or(PreferenceKeys::ADDRESSES)?,
            secret: get(PreferenceKeys::SECRET)
                .and_then(|value| value.downcast_into::<CFString>())
                .map(|string| string.to_string())
                .ok_or(PreferenceKeys::SECRET)?,
            allowed_sources,
        })
    }

    fn to_property_lists(&self) -> Vec<(&'static str, CFPropertyList)> {
        vec![
            (
                PreferenceKeys::PORT,
                CFNumber::from(self.port_number as i32).into_CFPropertyList(),
            ),
            (
                PreferenceKeys::ADDRESSES,
                addresses_to_property_list(&self.addresses),
            ),
            (
                PreferenceKeys::SECRET,
                CFString::new(&self.secret).into_CFPropertyList(),
            ),
            (
                PreferenceKeys::ALLOWED_SOURCES,
                addresses_to_property_list(&self.allowed_sources),
            ),
        ]
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
            addresses: parse_optional_addresses(&ips_string).map_err(|_error| {
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

/// A new installation accepts connections from any client on every interface, so each one gets its own
/// random secret – a shared default secret would let anyone on the network shut it down.
impl Default for AppConfiguration {
    fn default() -> Self {
        AppConfiguration {
            port_number: 53632,
            addresses: Vec::new(),
            secret: generate_secret(),
            allowed_sources: Vec::new(),
        }
    }
}

/// Generates a secret from 128 random bits, hex-encoded so that it's easy to type into a control system.
fn generate_secret() -> String {
    let mut bytes = [0u8; 16];
    // Reads the operating system's cryptographically secure random number generator
    getrandom::fill(&mut bytes).expect("the system random number generator is unavailable");
    bytes.iter().map(|byte| format!("{:02x}", byte)).collect()
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

/// The preferences domain – the same as the launchd label, so the settings live in
/// `/Library/Preferences/com.jkmassel.shutdownonlan.plist`, and can be managed with a configuration profile.
#[cfg(target_os = "macos")]
const PREFERENCES_DOMAIN: &str = "com.jkmassel.shutdownonlan";

#[cfg(target_os = "macos")]
const PREFERENCES_FILE: &str = "/Library/Preferences/com.jkmassel.shutdownonlan.plist";

/// Where versions before the move to `CFPreferences` stored their configuration.
#[cfg(target_os = "macos")]
const LEGACY_CONFIGURATION_FILE: &str =
    "/Library/Application Support/ShutdownOnLan/ShutDownOnLan.plist";

/// The same names that the legacy plist and the Windows registry values use.
#[cfg(target_os = "macos")]
struct PreferenceKeys {}

#[cfg(target_os = "macos")]
impl PreferenceKeys {
    const PORT: &'static str = "port_number";
    const ADDRESSES: &'static str = "addresses";
    const SECRET: &'static str = "secret";
    const ALLOWED_SOURCES: &'static str = "allowed_sources";
}

#[cfg(target_os = "macos")]
fn property_list_to_u16(value: CFPropertyList) -> Option<u16> {
    let number = value.downcast_into::<CFNumber>()?.to_i64()?;
    u16::try_from(number).ok()
}

#[cfg(target_os = "macos")]
fn property_list_to_addresses(value: CFPropertyList) -> Option<Vec<IpAddr>> {
    value
        .downcast_into::<CFArray>()?
        .get_all_values()
        .into_iter()
        .map(|item| {
            let item = unsafe { CFType::wrap_under_get_rule(item) };
            item.downcast::<CFString>()?.to_string().parse().ok()
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn addresses_to_property_list(addresses: &[IpAddr]) -> CFPropertyList {
    let strings: Vec<CFString> = addresses
        .iter()
        .map(|ip| CFString::new(&ip.to_string()))
        .collect();
    CFArray::from_CFTypes(&strings)
        .into_untyped()
        .into_CFPropertyList()
}

/// A preferences domain. Reads go through the standard search list, so values managed by a configuration
/// profile take precedence. Writes go to `user`, for any host.
#[cfg(target_os = "macos")]
struct Preferences {
    application_id: CFString,
    user: CFStringRef,
    /// The file `cfprefsd` stores the domain in, if it should be restricted to its owner.
    private_file: Option<&'static str>,
}

#[cfg(target_os = "macos")]
impl Preferences {
    /// The system-wide domain the service reads. Writing to it requires root.
    fn system() -> Preferences {
        Preferences {
            application_id: CFString::new(PREFERENCES_DOMAIN),
            user: unsafe { core_foundation_sys::preferences::kCFPreferencesAnyUser },
            private_file: Some(PREFERENCES_FILE),
        }
    }

    fn get(&self, key: &str) -> Option<CFPropertyList> {
        let key = CFString::new(key);
        let value = unsafe {
            core_foundation_sys::preferences::CFPreferencesCopyAppValue(
                key.as_concrete_TypeRef(),
                self.application_id.as_concrete_TypeRef(),
            )
        };

        if value.is_null() {
            None
        } else {
            Some(unsafe { CFPropertyList::wrap_under_create_rule(value) })
        }
    }

    fn set(&self, key: &str, value: &CFPropertyList) {
        let key = CFString::new(key);
        unsafe {
            core_foundation_sys::preferences::CFPreferencesSetValue(
                key.as_concrete_TypeRef(),
                value.as_concrete_TypeRef(),
                self.application_id.as_concrete_TypeRef(),
                self.user,
                core_foundation_sys::preferences::kCFPreferencesAnyHost,
            )
        }
    }

    /// Whether `key` is managed by a configuration profile.
    fn is_forced(&self, key: &str) -> bool {
        let key = CFString::new(key);
        unsafe {
            core_foundation_sys::preferences::CFPreferencesAppValueIsForced(
                key.as_concrete_TypeRef(),
                self.application_id.as_concrete_TypeRef(),
            ) != 0
        }
    }

    /// Writes pending changes to disk. This is where writing without permission fails.
    fn synchronize(&self) -> Result<(), ConfigurationError> {
        let succeeded = unsafe {
            core_foundation_sys::preferences::CFPreferencesSynchronize(
                self.application_id.as_concrete_TypeRef(),
                self.user,
                core_foundation_sys::preferences::kCFPreferencesAnyHost,
            ) != 0
        };

        if !succeeded {
            return Err(ConfigurationError::PreferencesNotWritable);
        }

        self.restrict_permissions()
    }

    /// Makes the file readable only by its owner – it holds the secret, which any local user could
    /// otherwise read. `cfprefsd` creates it (and `defaults write` may recreate it) as world-readable.
    fn restrict_permissions(&self) -> Result<(), ConfigurationError> {
        use std::os::unix::fs::PermissionsExt;

        let path = match self.private_file {
            Some(path) if Path::new(path).exists() => path,
            _ => return Ok(()),
        };

        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|error| {
            ConfigurationError::ConfigurationFileUnwritable {
                source: error,
                path: path.to_string(),
            }
        })
    }
}

/// Replaces the contents of `path` with `contents`, leaving the file readable only by its owner – it
/// holds the secret, which any local user could otherwise read.
#[cfg(target_os = "linux")]
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
    CorruptConfigurationFile,

    #[cfg(target_os = "macos")]
    #[error("The {0} preference is missing or invalid")]
    PreferenceNotReadable(&'static str),

    #[cfg(target_os = "macos")]
    #[error("Unable to save preferences – changing the configuration requires sudo")]
    PreferencesNotWritable,

    #[cfg(target_os = "macos")]
    #[error(
        "The {0} preference is managed by a configuration profile, so it can't be changed here"
    )]
    PreferenceIsManaged(&'static str),

    #[cfg(target_os = "linux")]
    #[error("Contents of Configuration File Are Invalid")]
    CorruptTomlConfigurationFile(#[source] toml::de::Error),

    #[cfg(target_os = "linux")]
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

    #[cfg(target_os = "linux")]
    #[error("Unable to write to configuration storage directory")]
    ConfigurationStorageUnwritable {
        source: std::io::Error,
        path: String,
    },

    #[cfg(not(windows))]
    #[error("Unable to write to configuration file at {path}")]
    ConfigurationFileUnwritable {
        source: std::io::Error,
        path: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "macos")]
    use std::path::PathBuf;

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
        configuration.set_addresses("10.0.1.100").unwrap();

        assert!(configuration
            .set_addresses("10.0.1.100,10.0.1.300")
            .is_err());
        assert_eq!(
            configuration.addresses,
            vec!["10.0.1.100".parse::<IpAddr>().unwrap()]
        );

        configuration.set_addresses("").unwrap();
        assert!(configuration.addresses.is_empty());
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
    fn test_default_configuration_accepts_connections_on_every_interface() {
        let configuration = AppConfiguration::default();

        assert!(configuration.accepts_connections_on(&"127.0.0.1".parse().unwrap()));
        assert!(configuration.accepts_connections_on(&"10.0.1.100".parse().unwrap()));
    }

    #[test]
    fn test_addresses_only_accept_connections_on_listed_interfaces() {
        let mut configuration = AppConfiguration::default();
        configuration.set_addresses("10.0.1.100").unwrap();

        assert!(configuration.accepts_connections_on(&"10.0.1.100".parse().unwrap()));
        assert!(!configuration.accepts_connections_on(&"192.168.1.100".parse().unwrap()));
    }

    #[test]
    fn test_each_default_configuration_has_its_own_random_secret() {
        let first = AppConfiguration::default().secret;
        let second = AppConfiguration::default().secret;

        assert_ne!(first, second);
        assert_eq!(first.len(), 32);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_debug_output_does_not_include_the_secret() {
        let configuration = AppConfiguration::default();
        let output = format!("{:?}", configuration);
        assert!(!output.contains(&configuration.secret));
    }

    #[cfg(target_os = "linux")]
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

    /// A preferences domain for the current user (so no root is needed) that's deleted when the test
    /// finishes.
    #[cfg(target_os = "macos")]
    struct TestPreferences {
        preferences: Preferences,
        domain: String,
        directory: PathBuf,
    }

    #[cfg(target_os = "macos")]
    impl TestPreferences {
        fn new(name: &str) -> TestPreferences {
            let domain = format!(
                "{}.test-{}-{}",
                PREFERENCES_DOMAIN,
                std::process::id(),
                name
            );
            let directory = std::env::temp_dir().join(&domain);
            std::fs::create_dir_all(&directory).unwrap();

            TestPreferences {
                preferences: Preferences {
                    application_id: CFString::new(&domain),
                    user: unsafe { core_foundation_sys::preferences::kCFPreferencesCurrentUser },
                    private_file: None,
                },
                domain,
                directory,
            }
        }

        fn remove(&self, key: &str) {
            let key = CFString::new(key);
            unsafe {
                core_foundation_sys::preferences::CFPreferencesSetValue(
                    key.as_concrete_TypeRef(),
                    std::ptr::null(),
                    self.preferences.application_id.as_concrete_TypeRef(),
                    self.preferences.user,
                    core_foundation_sys::preferences::kCFPreferencesAnyHost,
                )
            }
        }

        fn legacy_file(&self, contents: &str) -> PathBuf {
            let path = self.directory.join("ShutDownOnLan.plist");
            std::fs::write(&path, contents).unwrap();
            path
        }

        fn custom_configuration() -> AppConfiguration {
            let mut configuration = AppConfiguration {
                port_number: 12345,
                ..AppConfiguration::default()
            };
            configuration.set_addresses("10.0.1.100,::1").unwrap();
            configuration
                .set_secret("custom secret".to_string())
                .unwrap();
            configuration.set_allowed_sources("10.0.1.50").unwrap();
            configuration
        }
    }

    #[cfg(target_os = "macos")]
    impl Drop for TestPreferences {
        fn drop(&mut self) {
            for (key, _) in AppConfiguration::default().to_property_lists() {
                self.remove(key);
            }
            let _ = self.preferences.synchronize();
            let _ = std::fs::remove_dir_all(&self.directory);

            // An emptied domain still leaves a file behind
            if let Some(home) = std::env::var_os("HOME") {
                let _ = std::fs::remove_file(
                    PathBuf::from(home)
                        .join("Library/Preferences")
                        .join(format!("{}.plist", self.domain)),
                );
            }
        }
    }

    #[cfg(target_os = "macos")]
    const LEGACY_PLIST_WITHOUT_ALLOWED_SOURCES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>port_number</key>
	<integer>12345</integer>
	<key>addresses</key>
	<array>
		<string>10.0.1.100</string>
		<string>::1</string>
	</array>
	<key>secret</key>
	<string>custom secret</string>
</dict>
</plist>"#;

    #[cfg(target_os = "macos")]
    #[test]
    fn test_preferences_round_trip() {
        let test = TestPreferences::new("round-trip");
        let configuration = TestPreferences::custom_configuration();

        configuration.save_to(&test.preferences).unwrap();

        assert_eq!(
            AppConfiguration::fetch_from(&test.preferences).unwrap(),
            configuration
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_empty_preferences_are_filled_with_defaults() {
        let test = TestPreferences::new("empty");

        AppConfiguration::write_missing_defaults(&test.preferences).unwrap();

        // Every default configuration has a different random secret, so compare everything else
        let configuration = AppConfiguration::fetch_from(&test.preferences).unwrap();
        assert_eq!(configuration.secret.len(), 32);
        assert_eq!(
            configuration,
            AppConfiguration {
                secret: configuration.secret.clone(),
                ..AppConfiguration::default()
            }
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_only_missing_preferences_are_restored() {
        let test = TestPreferences::new("missing-value");
        let configuration = TestPreferences::custom_configuration();

        configuration.save_to(&test.preferences).unwrap();
        test.remove(PreferenceKeys::PORT);

        AppConfiguration::write_missing_defaults(&test.preferences).unwrap();

        assert_eq!(
            AppConfiguration::fetch_from(&test.preferences).unwrap(),
            AppConfiguration {
                port_number: AppConfiguration::default().port_number,
                ..configuration
            }
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_invalid_preferences_are_reported() {
        let test = TestPreferences::new("invalid-value");
        TestPreferences::custom_configuration()
            .save_to(&test.preferences)
            .unwrap();

        // The port should be a number
        test.preferences.set(
            PreferenceKeys::PORT,
            &CFString::new("not a number").into_CFPropertyList(),
        );

        assert!(matches!(
            AppConfiguration::fetch_from(&test.preferences),
            Err(ConfigurationError::PreferenceNotReadable(
                PreferenceKeys::PORT
            ))
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_legacy_file_is_migrated_and_removed() {
        let test = TestPreferences::new("migrate");
        let path = test.legacy_file(LEGACY_PLIST_WITHOUT_ALLOWED_SOURCES);

        AppConfiguration::migrate_legacy_file(&path, &test.preferences).unwrap();

        let migrated = AppConfiguration::fetch_from(&test.preferences).unwrap();
        assert_eq!(
            migrated,
            AppConfiguration {
                allowed_sources: Vec::new(),
                ..TestPreferences::custom_configuration()
            }
        );
        assert!(migrated.accepts_connections_from(&"10.0.1.99".parse().unwrap()));
        assert!(!path.exists());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_legacy_file_replaces_existing_preferences() {
        let test = TestPreferences::new("migrate-over-defaults");
        AppConfiguration::write_missing_defaults(&test.preferences).unwrap();
        let path = test.legacy_file(LEGACY_PLIST_WITHOUT_ALLOWED_SOURCES);

        AppConfiguration::migrate_legacy_file(&path, &test.preferences).unwrap();

        assert_eq!(
            AppConfiguration::fetch_from(&test.preferences)
                .unwrap()
                .port_number,
            12345
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_corrupt_legacy_file_is_kept() {
        let test = TestPreferences::new("migrate-corrupt");
        let path = test.legacy_file("not a plist");

        assert!(matches!(
            AppConfiguration::migrate_legacy_file(&path, &test.preferences),
            Err(ConfigurationError::CorruptConfigurationFile)
        ));
        assert!(path.exists());
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

        // Every default configuration has a different random secret, so compare everything else
        let configuration = AppConfiguration::fetch_from(&test.registry).unwrap();
        assert_eq!(configuration.secret.len(), 32);
        assert_eq!(
            configuration,
            AppConfiguration {
                secret: configuration.secret.clone(),
                ..AppConfiguration::default()
            }
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
