#pragma once

/// @file
/// The configurations read from a file: `ConfigContext`, the process's own when no node
/// loads it, and `ConfigFile`, a TOML file read through `ConfigView` apart from it.
/// config.h includes this header.

#include "hypha/config.h"

#include <string>

namespace hypha {

/// A standalone parameter configuration scope that does not join the bus.
class ConfigContext
{
  public:
    explicit ConfigContext(const std::string& parameterFile)
    {
        bool         created = false;
        std::string  refusal;
        const Status status = backend::initConfig(parameterFile, &created, &refusal);
        this->ownsConfig_ = created;
        if (status != Status::Ok) {
            const std::string context = "configuration '" + parameterFile + "' loading";
            throw std::runtime_error(detail::failureText(context, status, refusal));
        }
    }

    ~ConfigContext()
    {
        if (!this->ownsConfig_) {
            return;
        }
        backend::shutdownConfig();
    }

    ConfigContext(const ConfigContext&) = delete;
    ConfigContext& operator=(const ConfigContext&) = delete;
    ConfigContext(ConfigContext&&) = delete;
    ConfigContext& operator=(ConfigContext&&) = delete;

  private:
    bool ownsConfig_{false};
};

/// A TOML file read through ConfigView apart from the configuration of the process, such
/// as a camera calibration: the parameter-file loader reads it (`include`, `${NAME}`), its
/// reads mark its keys read for unreadKeys(), and they are not recorded as parameters of
/// the node. path() resolves against the directory of the file. The views borrow the file:
/// they must not outlive it.
class ConfigFile
{
  public:
    /// @throws ConfigError naming @p path when the file is missing, unreadable or not a
    ///         parameter file the loader accepts (it logs the reason, with the line).
    explicit ConfigFile(const std::string& path)
    {
        const Status status = backend::loadConfigFile(path, &this->config_);
        if (status != Status::Ok) {
            throw ConfigError(
                "hypha: '" + path + "' cannot be read as a TOML file (the reason is logged above)"
            );
        }
    }

    ~ConfigFile()
    {
        backend::destroyConfigFile(this->config_);
    }

    ConfigFile(const ConfigFile&) = delete;
    ConfigFile& operator=(const ConfigFile&) = delete;
    ConfigFile(ConfigFile&&) = delete;
    ConfigFile& operator=(ConfigFile&&) = delete;

    /// @brief The table @p prefix of the file, or the whole file.
    ConfigView view(const std::string& prefix = {}) const
    {
        return ConfigView(this->config_, prefix);
    }

  private:
    HyphaConfig* config_{nullptr};
};

}  // namespace hypha
