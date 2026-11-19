#pragma once

/// @file
/// PLUGINLIB_EXPORT_CLASS registers a factory in mrs::PluginRegistry<Base> under the
/// class's upstream address ("mrs_uav_trackers/LineTracker"). A manager creates a
/// plugin with mrs::PluginRegistry<Base>::create(address).

#include <functional>
#include <map>
#include <memory>
#include <stdexcept>
#include <string>

namespace mrs {

template <class Base>
class PluginRegistry
{
  public:
    using Factory = std::function<std::unique_ptr<Base>()>;

    static bool add(const std::string& address, Factory factory)
    {
        return factories().emplace(address, std::move(factory)).second;
    }

    /// @throws std::invalid_argument for an address nobody registered.
    static std::unique_ptr<Base> create(const std::string& address)
    {
        const auto found = factories().find(address);
        if (found == factories().end()) {
            throw std::invalid_argument("mrs: no plugin registered as '" + address + "'");
        }
        return found->second();
    }

  private:
    static std::map<std::string, Factory>& factories()
    {
        static std::map<std::string, Factory> map;
        return map;
    }
};

namespace detail {
/// "mrs_uav_trackers::line_tracker::LineTracker" -> "mrs_uav_trackers/LineTracker".
inline std::string pluginAddress(std::string qualified)
{
    while (!qualified.empty() && qualified.front() == ':') {
        qualified.erase(0, 1);
    }
    const size_t first = qualified.find("::");
    const size_t last = qualified.rfind("::");
    if (first == std::string::npos) {
        return qualified;
    }
    return qualified.substr(0, first) + "/" + qualified.substr(last + 2);
}
}  // namespace detail

}  // namespace mrs

#define MRS_PLUGIN_CAT2(a, b) a##b
#define MRS_PLUGIN_CAT(a, b) MRS_PLUGIN_CAT2(a, b)
#define PLUGINLIB_EXPORT_CLASS(Class, Base)                                                        \
    namespace {                                                                                    \
    [[maybe_unused]] const bool MRS_PLUGIN_CAT(mrsPluginRegistered, __LINE__) =                    \
        ::mrs::PluginRegistry<Base>::add(::mrs::detail::pluginAddress(#Class), [] {                \
            return std::unique_ptr<Base>(new Class());                                             \
        });                                                                                        \
    }
