#pragma once

/// @file
/// The C ABI of services, as the typed facade (hypha/service.h) uses it: the handle
/// owners, the declaration record, and the refusal of a create as an exception. Every rule
/// of a call is the library's; this layer only carries bytes and callbacks across.

#include "hypha/backend/handles.h"
#include "hypha/backend/session.h"
#include "hypha/backend/text_readers.h"
#include "hypha/hypha_ffi.h"
#include "hypha/status.h"
#include "hypha_base/clock.h"

#include <algorithm>
#include <cstdint>
#include <stdexcept>
#include <string>
#include <string_view>

namespace hypha {

/// @brief The limits of a service, which its server and its clients declare alike. A policy
///        with an empty name is `defaultService`: 8 clients (and 3 for tools), 2 calls in
///        flight each, 4 KiB each way, a timeout of 1 s. A named policy sets every field.
struct ServicePolicy
{
    std::string name;
    uint32_t    clients{0};
    uint32_t    outstanding{0};
    uint64_t    requestBytes{0};
    uint64_t    responseBytes{0};
    Duration    timeout{};
};

/// @brief The policy both ends of a service of type @p Srv declare: `defaultService` unless
///        a header that both ends include specializes it.
template <typename Srv>
struct ServicePolicyOf
{
    static inline const ServicePolicy value{};
};

}  // namespace hypha

namespace hypha::backend {

template <>
struct HandleRelease<HyphaServer>
{
    static void release(HyphaServer* handle)
    {
        hypha_service_server_destroy(handle);
    }
};

template <>
struct HandleRelease<HyphaClient>
{
    static void release(HyphaClient* handle)
    {
        hypha_service_client_destroy(handle);
    }
};

/// A deferred request: its release refuses it when it is still unanswered.
template <>
struct HandleRelease<HyphaRequest>
{
    static void release(HyphaRequest* handle)
    {
        hypha_request_destroy(handle);
    }
};

using UniqueServer = UniqueHandle<HyphaServer>;
using UniqueClient = UniqueHandle<HyphaClient>;
using UniqueRequest = UniqueHandle<HyphaRequest>;

/// What a server or a client is declared with: a port of the ports tables, or a literal
/// service name.
struct ServiceDeclaration
{
    std::string   name;
    bool          literal{false};
    std::string   typeName;
    std::string   md5;
    ServicePolicy policy;
    Duration      timeout{};
    bool          blocking{false};

    /// What a failure names: `ports.servers.<port>` or the service.
    std::string describe(const char* table) const
    {
        return this->literal ? "service '" + this->name + "'"
                             : std::string("ports.") + table + "." + this->name;
    }
};

/// The C record of @p declaration, which borrows its texts.
inline HyphaServiceDesc descriptionOf(const ServiceDeclaration& declaration)
{
    HyphaServiceDesc desc{};
    desc.name = declaration.name.data();
    desc.name_len = declaration.name.size();
    desc.literal = declaration.literal ? 1 : 0;
    desc.type_name = declaration.typeName.data();
    desc.type_name_len = declaration.typeName.size();
    desc.md5 = declaration.md5.data();
    desc.md5_len = declaration.md5.size();
    const ServicePolicy& policy = declaration.policy;
    desc.policy.name = policy.name.data();
    desc.policy.name_len = policy.name.size();
    desc.policy.clients = policy.clients;
    desc.policy.outstanding = policy.outstanding;
    desc.policy.request_bytes = policy.requestBytes;
    desc.policy.response_bytes = policy.responseBytes;
    desc.policy.timeout_nanos = static_cast<uint64_t>(std::max<int64_t>(policy.timeout.nanos(), 0));
    desc.timeout_nanos = static_cast<uint64_t>(std::max<int64_t>(declaration.timeout.nanos(), 0));
    desc.blocking = declaration.blocking ? 1 : 0;
    return desc;
}

/// @throws ConfigError for a port whose table is missing or refused, or a policy mismatch;
///         std::runtime_error for any other refusal. The text carries the library's reason.
[[noreturn]] inline void
throwServiceFailure(const ServiceDeclaration& declaration, const char* table, Status status)
{
    const std::string text =
        detail::failureText(declaration.describe(table), status, lastError());
    const bool configuration = status == Status::QosMismatch
                            || (!declaration.literal && status == Status::InvalidArgument);
    if (configuration) {
        throw ConfigError(text);
    }
    throw std::runtime_error(text);
}

/// The process node, or an exception when no named Node exists yet.
inline HyphaNode* serviceNode(const ServiceDeclaration& declaration, const char* table)
{
    HyphaNode* node = nodeHandle();
    if (node == nullptr) {
        throw std::runtime_error(
            detail::failureText(declaration.describe(table), Status::InvalidState)
            + ": no session exists; create a named hypha::Node first"
        );
    }
    return node;
}

/// Create the server of @p declaration; @p serve runs with @p context for each request,
/// and @p release runs once with it, also when the create is refused.
inline UniqueServer createServer(
    const ServiceDeclaration& declaration,
    HyphaServeCallback        serve,
    void*                     context,
    HyphaContextRelease       release
)
{
    HyphaNode* node = nullptr;
    try {
        node = serviceNode(declaration, "servers");
    } catch (...) {
        if (release != nullptr) {
            release(context);
        }
        throw;
    }
    const HyphaServiceDesc desc = descriptionOf(declaration);
    UniqueServer           server;
    const Status           status =
        hypha_service_server_create(node, &desc, serve, context, release, server.put());
    if (status != Status::Ok) {
        throwServiceFailure(declaration, "servers", status);
    }
    return server;
}

inline UniqueClient createClient(const ServiceDeclaration& declaration)
{
    HyphaNode*             node = serviceNode(declaration, "clients");
    const HyphaServiceDesc desc = descriptionOf(declaration);
    UniqueClient           client;
    const Status           status = hypha_service_client_create(node, &desc, client.put());
    if (status != Status::Ok) {
        throwServiceFailure(declaration, "clients", status);
    }
    return client;
}

/// The library's status of @p client: whether a live server exists, and pending calls.
inline HyphaClientStatus clientStatus(const HyphaClient* client)
{
    HyphaClientStatus status{};
    if (client != nullptr) {
        require(Status(hypha_client_status(client, &status)), "service client status");
    }
    return status;
}

/// The text of a refused call, with the library's reason.
[[noreturn]] inline void throwCallFailure(std::string_view what, Status status)
{
    throw std::runtime_error(detail::failureText(what, status, lastError()));
}

}  // namespace hypha::backend
