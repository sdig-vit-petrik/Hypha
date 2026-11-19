#pragma once

/// @file
/// The session: the process node, its default WaitSet and the run loop.

#include "hypha/backend/abi.h"
#include "hypha/backend/callbacks.h"
#include "hypha/backend/session_state.h"
#include "hypha/backend/text_readers.h"
#include "hypha/detail/report.h"
#include "hypha/hypha_ffi.h"
#include "hypha/status.h"
#include "hypha_base/clock.h"

#include <cstdint>
#include <expected>
#include <iostream>
#include <memory>
#include <mutex>
#include <optional>
#include <string>
#include <vector>

namespace hypha::backend {

/// @brief Create the process's node, or do nothing when it exists. Concurrent callers
///        create one node.
/// @param created Set to true only when this call created the node.
inline Status
init(const std::string& nodeName, const std::string& parameterFile, bool* created = nullptr)
{
    std::string abiDiagnostic;
    if (!verifyAbi(&abiDiagnostic)) {
        std::cerr << abiDiagnostic << '\n';
        return Status::InvalidState;
    }
    ffi::Session&                     state = ffi::session();
    const std::lock_guard<std::mutex> lock(state.mutex);
    if (state.shuttingDown) {
        return Status::InvalidState;
    }
    if (state.node != nullptr) {
        const bool sameFile = parameterFile == state.parameterFile;
        return sameFile ? Status::Ok : Status::InvalidState;
    }
    const bool conflictingConfig = state.config != nullptr && parameterFile != state.parameterFile;
    if (conflictingConfig) {
        return Status::InvalidState;
    }
    HyphaNode*          node = nullptr;
    const ::HyphaStatus status = hypha_node_create(
        nodeName.data(),
        nodeName.size(),
        parameterFile.data(),
        parameterFile.size(),
        state.config,
        &node
    );
    if (status != HYPHA_STATUS_OK) {
        return Status(status);
    }
    const std::expected<std::string, Status> name = readNodeName(node);
    if (!name) {
        hypha_node_destroy(node);
        return Status::Backend;
    }
    state.name = *name;
    state.parameterFile = parameterFile;
    state.node = node;
    if (created != nullptr) {
        *created = true;
    }
    return Status::Ok;
}

inline Status initConfig(const std::string& parameterFile, bool* created = nullptr)
{
    std::string abiDiagnostic;
    if (!verifyAbi(&abiDiagnostic)) {
        std::cerr << abiDiagnostic << '\n';
        return Status::InvalidState;
    }
    ffi::Session&                     state = ffi::session();
    const std::lock_guard<std::mutex> lock(state.mutex);
    if (state.shuttingDown) {
        return Status::InvalidState;
    }
    const bool alreadyConfigured = state.node != nullptr || state.config != nullptr;
    if (alreadyConfigured) {
        const bool sameFile = parameterFile == state.parameterFile;
        return sameFile ? Status::Ok : Status::InvalidState;
    }
    HyphaConfig*        config = nullptr;
    const ::HyphaStatus status =
        hypha_config_create(parameterFile.data(), parameterFile.size(), &config);
    if (status != HYPHA_STATUS_OK) {
        return Status(status);
    }
    state.parameterFile = parameterFile;
    state.config = config;
    if (created != nullptr) {
        *created = true;
    }
    return Status::Ok;
}

inline void shutdownConfig()
{
    ffi::Session& state = ffi::session();
    HyphaConfig*  config = nullptr;
    {
        const std::lock_guard<std::mutex> lock(state.mutex);
        config = state.config;
        state.config = nullptr;
        if (state.node == nullptr) {
            state.parameterFile.clear();
        }
    }
    if (config != nullptr) {
        hypha_config_destroy(config);
    }
}

/// Leaves the bus, destroying every cycle. A step released here that calls back finds no
/// node and does nothing.
inline void shutdown()
{
    ffi::Session& state = ffi::session();
    HyphaNode*    node = nullptr;
    {
        const std::lock_guard<std::mutex> lock(state.mutex);
        if (state.shuttingDown) {
            return;
        }
        state.shuttingDown = true;
        node = state.node;
        state.node = nullptr;
        ++state.generation;
    }
    if (node != nullptr) {
        hypha_node_shutdown(node);
        hypha_node_destroy(node);
    }
    {
        const std::lock_guard<std::mutex> lock(state.mutex);
        state.name.clear();
        if (state.config == nullptr) {
            state.parameterFile.clear();
        }
        state.shuttingDown = false;
    }
}

/// The number of sessions this process has released.
inline uint64_t sessionGeneration()
{
    ffi::Session&                     state = ffi::session();
    const std::lock_guard<std::mutex> lock(state.mutex);
    return state.generation;
}

inline bool ok()
{
    ffi::Session&                     state = ffi::session();
    const std::lock_guard<std::mutex> lock(state.mutex);
    return state.node != nullptr && hypha_node_ok(state.node) != 0;
}

inline std::string nodeName()
{
    ffi::Session&                     state = ffi::session();
    const std::lock_guard<std::mutex> lock(state.mutex);
    return state.name;
}

/// The node this process joined the bus with, or null before a Node exists.
inline HyphaNode* nodeHandle()
{
    ffi::Session& state = ffi::session();
    HyphaNode*    node = nullptr;
    {
        const std::lock_guard<std::mutex> lock(state.mutex);
        node = state.node;
    }
    return node;
}

/// The active parameter configuration: the Node's configuration when a Node exists,
/// else the standalone ConfigContext handle, or null.
inline const HyphaConfig* configHandle()
{
    ffi::Session&                     state = ffi::session();
    const std::lock_guard<std::mutex> lock(state.mutex);
    if (state.node != nullptr) {
        return hypha_node_config(state.node);
    }
    return state.config;
}

/// The node's WaitSet, owned by the library; null before the session exists.
inline HyphaWaitSet* defaultWaitSet()
{
    ffi::Session& state = ffi::session();
    HyphaNode*    node = nullptr;
    {
        const std::lock_guard<std::mutex> lock(state.mutex);
        node = state.node;
    }
    if (node == nullptr) {
        return nullptr;
    }
    HyphaWaitSet* waitset = nullptr;
    if (hypha_node_default_waitset(node, &waitset) != HYPHA_STATUS_OK) {
        return nullptr;
    }
    return waitset;
}

/// Runs the node's default WaitSet on the calling thread until it stops.
inline RunResult run()
{
    HyphaWaitSet* waitset = defaultWaitSet();
    if (waitset == nullptr) {
        return RunResult::apiFailure(HYPHA_STATUS_INVALID_STATE);
    }
    HyphaWaitSetRunReport report{};
    // The session mutex is not held here: the run blocks until it is stopped.
    const Status status = hypha_waitset_run(waitset, 0, &report);
    if (status != HYPHA_STATUS_OK) {
        return RunResult::apiFailure(status);
    }
    return RunResult::fromReport(report);
}

/// Runs @p waitset for at most @p limit, then returns. A limit of zero or less is
/// refused: the C ABI takes it unsigned.
inline RunResult runWaitSetFor(HyphaWaitSet* waitset, Duration limit)
{
    if (waitset == nullptr) {
        return RunResult::apiFailure(HYPHA_STATUS_INVALID_STATE);
    }
    if (limit.nanos() <= 0) {
        return RunResult::apiFailure(HYPHA_STATUS_INVALID_ARGUMENT);
    }
    HyphaWaitSetRunReport report{};
    const Status status = hypha_waitset_run(waitset, static_cast<uint64_t>(limit.nanos()), &report);
    if (status != HYPHA_STATUS_OK) {
        return RunResult::apiFailure(status);
    }
    return RunResult::fromReport(report);
}

/// Runs the node's default WaitSet for at most `limit`, then returns.
inline RunResult runFor(Duration limit)
{
    return runWaitSetFor(defaultWaitSet(), limit);
}

/// Asks the node's default WaitSet to return. Does nothing before a Node exists.
inline void runStop()
{
    HyphaWaitSet* waitset = defaultWaitSet();
    if (waitset == nullptr) {
        return;
    }
    hypha_waitset_request_stop(waitset);
}

/// Asks the node's default WaitSet to end its run as failed. Does nothing before a Node
/// exists.
inline void runFail()
{
    HyphaWaitSet* waitset = defaultWaitSet();
    if (waitset == nullptr) {
        return;
    }
    hypha_waitset_request_failure(waitset);
}

}  // namespace hypha::backend
