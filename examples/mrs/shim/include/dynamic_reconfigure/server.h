#pragma once

/// @file
/// dynamic_reconfigure::Server without a server: it holds a Config. updateConfig()
/// stores it; setCallback() stores the callback and never calls it.

#include <functional>
#include <memory>
#include <mutex>

namespace dynamic_reconfigure {

template <class Config>
class Server
{
  public:
    using CallbackType = std::function<void(Config&, uint32_t)>;

    Server() = default;

    template <class Mutex, class NodeHandle>
    Server(Mutex&, const NodeHandle&)
    {
    }

    template <class NodeHandle>
    explicit Server(const NodeHandle&)
    {
    }

    void updateConfig(const Config& config)
    {
        this->config_ = config;
    }

    void setCallback(const CallbackType& callback)
    {
        this->callback_ = callback;
    }

    const Config& config() const
    {
        return this->config_;
    }

  private:
    Config       config_{};
    CallbackType callback_;
};

}  // namespace dynamic_reconfigure
