#pragma once

/// @file
/// Move-only owners of the C handles. A cycle has none: its WaitSet owns it.

#include "hypha/detail/report.h"
#include "hypha/hypha_ffi.h"
#include "hypha/status.h"

#include <cstdio>
#include <string>

namespace hypha::backend {

/// Release function of one C handle type. Each handle below specializes it.
template <typename Handle>
struct HandleRelease;

/// Move-only owner of one C handle. The destructor releases a handle it still holds.
template <typename Handle>
class UniqueHandle
{
  public:
    UniqueHandle() = default;

    explicit UniqueHandle(Handle* owned) : handle(owned)
    {
    }

    UniqueHandle(const UniqueHandle&) = delete;
    UniqueHandle& operator=(const UniqueHandle&) = delete;

    UniqueHandle(UniqueHandle&& other) noexcept : handle(other.release())
    {
    }

    UniqueHandle& operator=(UniqueHandle&& other) noexcept
    {
        if (this != &other) {
            this->reset(other.release());
        }
        return *this;
    }

    ~UniqueHandle()
    {
        this->reset();
    }

    Handle* get() const
    {
        return this->handle;
    }

    explicit operator bool() const
    {
        return this->handle != nullptr;
    }

    /// Give up ownership. The caller must release the returned handle.
    Handle* release()
    {
        Handle* given = this->handle;
        this->handle = nullptr;
        return given;
    }

    /// Release the held handle, then hold @p next.
    void reset(Handle* next = nullptr)
    {
        Handle* old = this->handle;
        this->handle = next;
        if (old != nullptr) {
            HandleRelease<Handle>::release(old);
        }
    }

    /// Release the held handle. The result is the slot for a C create function.
    Handle** put()
    {
        this->reset();
        return &this->handle;
    }

  private:
    Handle* handle{nullptr};
};

template <>
struct HandleRelease<HyphaLoan>
{
    static void release(HyphaLoan* handle)
    {
        hypha_loan_drop(handle);
    }
};

template <>
struct HandleRelease<HyphaPublisher>
{
    static void release(HyphaPublisher* handle)
    {
        hypha_publisher_destroy(handle);
    }
};

template <>
struct HandleRelease<HyphaSubscriber>
{
    static void release(HyphaSubscriber* handle)
    {
        hypha_subscriber_destroy(handle);
    }
};

/// A failed destroy is logged to stderr. A shared holder owns the source.
template <>
struct HandleRelease<HyphaInputSource>
{
    static void release(HyphaInputSource* handle)
    {
        const ::HyphaStatus status = hypha_input_source_destroy(handle);
        if (status != HYPHA_STATUS_OK) {
            std::fprintf(
                stderr,
                "hypha: input source cleanup failed with status %d\n",
                static_cast<int>(status)
            );
        }
    }
};

/// A destroy the library refuses (while the node's WaitSet runs on another thread) is
/// logged; the handle then lives until the node is destroyed.
template <>
struct HandleRelease<HyphaTransformBuffer>
{
    static void release(HyphaTransformBuffer* handle)
    {
        const ::HyphaStatus status = hypha_transform_buffer_destroy(handle);
        if (status != HYPHA_STATUS_OK) {
            hypha::detail::error(
                "a transform buffer could not be destroyed: status "
                + std::to_string(static_cast<int>(status))
                + "; destroy it on the node's thread while the node does not run"
            );
        }
    }
};

using UniqueInputSource = UniqueHandle<HyphaInputSource>;
using UniqueLoan = UniqueHandle<HyphaLoan>;
using UniquePublisher = UniqueHandle<HyphaPublisher>;
using UniqueSubscriber = UniqueHandle<HyphaSubscriber>;
using UniqueTransformBuffer = UniqueHandle<HyphaTransformBuffer>;

}  // namespace hypha::backend
