#pragma once

/// @file
/// hypha::Task and what a Task awaits: hypha::call(), hypha::sleepFor(), hypha::until(),
/// and another Task. A hypha::Sequence (hypha/sequence.h) runs a Task in the steps of a
/// cycle; nothing else resumes one.

#include "hypha/cycle.h"
#include "hypha/cycle_service.h"
#include "hypha/service.h"

#include <coroutine>
#include <cstdint>
#include <exception>
#include <optional>
#include <stdexcept>
#include <string>
#include <type_traits>
#include <utility>
#include <variant>

namespace hypha {

template <typename T>
class Task;

namespace detail {

/// What a waiting sequence waits on: polled at each advance until ready.
struct SequenceWait
{
    bool (*ready)(SequenceWait* self, Time now);
    const char*        what;
    const std::string* port{nullptr};
};

/// What a running sequence shares with its frames: the frame to resume and what it waits on.
struct SequenceCore
{
    std::coroutine_handle<> leaf;
    SequenceWait*           waiting{nullptr};
    Time                    now{};
};

template <typename T>
using TaskValue = std::conditional_t<std::is_void_v<T>, std::monostate, T>;

struct TaskPromiseBase
{
    SequenceCore*           core{nullptr};
    std::coroutine_handle<> continuation;
    std::exception_ptr      error;

    std::suspend_always initial_suspend() noexcept
    {
        return {};
    }

    struct Final
    {
        bool await_ready() noexcept
        {
            return false;
        }

        template <typename Promise>
        std::coroutine_handle<> await_suspend(std::coroutine_handle<Promise> done) noexcept
        {
            const std::coroutine_handle<> next = done.promise().continuation;
            return next ? next : std::noop_coroutine();
        }

        void await_resume() noexcept
        {
        }
    };

    Final final_suspend() noexcept
    {
        return {};
    }

    void unhandled_exception() noexcept
    {
        this->error = std::current_exception();
    }
};

template <typename T>
struct TaskPromise : TaskPromiseBase
{
    std::optional<T> value;

    void return_value(T result)
    {
        this->value.emplace(std::move(result));
    }
};

template <>
struct TaskPromise<void> : TaskPromiseBase
{
    std::optional<std::monostate> value;

    void return_void()
    {
        this->value.emplace();
    }
};

/// Suspend the frame @p leaf on @p wait.
template <typename Promise>
void waitOn(SequenceWait& wait, std::coroutine_handle<Promise> leaf)
{
    SequenceCore* core = leaf.promise().core;
    core->leaf = leaf;
    core->waiting = &wait;
}

template <typename T>
class SequenceRunner;

}  // namespace detail

/// @brief A coroutine of a hypha::Sequence that returns @p T. Move-only; it runs when a
///        Sequence starts it or another Task awaits it.
template <typename T = void>
class [[nodiscard]] Task
{
  public:
    struct promise_type : detail::TaskPromise<T>
    {
        Task get_return_object()
        {
            return Task(std::coroutine_handle<promise_type>::from_promise(*this));
        }
    };

    /// An empty Task, which no Sequence starts.
    Task() noexcept = default;

    Task(Task&& other) noexcept : frame_(std::exchange(other.frame_, {}))
    {
    }

    Task& operator=(Task&& other) noexcept
    {
        if (this != &other) {
            this->reset();
            this->frame_ = std::exchange(other.frame_, {});
        }
        return *this;
    }

    Task(const Task&) = delete;
    Task& operator=(const Task&) = delete;

    ~Task()
    {
        this->reset();
    }

    /// What awaits a Task in another Task: it runs in the awaiting sequence.
    struct Awaiter
    {
        std::coroutine_handle<promise_type> child;

        bool await_ready() const noexcept
        {
            return false;
        }

        template <typename Promise>
        std::coroutine_handle<> await_suspend(std::coroutine_handle<Promise> parent) noexcept
        {
            this->child.promise().core = parent.promise().core;
            this->child.promise().continuation = parent;
            return this->child;
        }

        T await_resume()
        {
            return Task::take(this->child.promise());
        }
    };

    /// Another Task awaits it, and gets its result.
    Awaiter operator co_await() && noexcept
    {
        return Awaiter{this->frame_};
    }

  private:
    template <typename>
    friend class detail::SequenceRunner;

    explicit Task(std::coroutine_handle<promise_type> frame) : frame_(frame)
    {
    }

    static T take(promise_type& promise)
    {
        if (promise.error) {
            std::rethrow_exception(promise.error);
        }
        if constexpr (!std::is_void_v<T>) {
            return std::move(*promise.value);
        }
    }

    void reset()
    {
        if (this->frame_) {
            std::exchange(this->frame_, {}).destroy();
        }
    }

    std::coroutine_handle<promise_type> frame_;
};

/// @brief Stage @p request on @p port; `co_await` resumes with the call's outcome.
template <typename Srv>
class [[nodiscard]] CallAwaiter : private detail::SequenceWait
{
  public:
    CallAwaiter(Calls<Srv>& port, const typename Srv::Request& request)
        : SequenceWait{&CallAwaiter::poll, "the outcome of a call to", &port.name()}
        , port_(&port)
        , request_(&request)
    {
    }

    bool await_ready() const noexcept
    {
        return false;
    }

    template <typename Promise>
    bool await_suspend(std::coroutine_handle<Promise> leaf)
    {
        const CallStatus status = this->port_->call(*this->request_);
        if (status != CallStatus::Sent) {
            this->result_.emplace(CallResult<Srv>::notSent(status));
            return false;
        }
        this->unread_ = this->port_->unreadResults();
        detail::waitOn(*this, leaf);
        return true;
    }

    CallResult<Srv> await_resume()
    {
        return std::move(*this->result_);
    }

  private:
    static bool poll(detail::SequenceWait* wait, Time)
    {
        auto* self = static_cast<CallAwaiter*>(wait);
        if (std::optional<CallResult<Srv>> result = self->port_->result()) {
            self->result_ = std::move(result);
            return true;
        }
        if (self->port_->unreadResults() != self->unread_) {
            throw std::logic_error(
                "hypha::Sequence: the outcome of the call to '" + self->port_->name()
                + "' was dropped by a step that did not advance the sequence"
            );
        }
        return false;
    }

    Calls<Srv>*                    port_;
    const typename Srv::Request*   request_;
    std::optional<CallResult<Srv>> result_;
    uint64_t                       unread_{0};
};

/// @brief `co_await hypha::call(port, request)` in a Task: the CallResult of the call.
template <typename Srv>
CallAwaiter<Srv> call(Calls<Srv>& port, const typename Srv::Request& request)
{
    return CallAwaiter<Srv>(port, request);
}

/// @brief Resumes at the first advance at least `d` of step time after it is reached.
class [[nodiscard]] SleepAwaiter : private detail::SequenceWait
{
  public:
    explicit SleepAwaiter(Duration delay)
        : SequenceWait{&SleepAwaiter::poll, "the end of a sleep"}, delay_(delay)
    {
    }

    bool await_ready() const noexcept
    {
        return this->delay_.nanos() <= 0;
    }

    template <typename Promise>
    void await_suspend(std::coroutine_handle<Promise> leaf)
    {
        this->until_ = leaf.promise().core->now + this->delay_;
        detail::waitOn(*this, leaf);
    }

    void await_resume() const noexcept
    {
    }

  private:
    static bool poll(detail::SequenceWait* wait, Time now)
    {
        return now >= static_cast<SleepAwaiter*>(wait)->until_;
    }

    Duration delay_;
    Time     until_{};
};

/// @brief `co_await hypha::sleepFor(d)` in a Task: wait `d` of step time.
inline SleepAwaiter sleepFor(Duration delay)
{
    return SleepAwaiter(delay);
}

/// @brief Resumes once its predicate holds: checked when reached and at each advance.
template <typename Pred>
class [[nodiscard]] UntilAwaiter : private detail::SequenceWait
{
  public:
    explicit UntilAwaiter(Pred pred)
        : SequenceWait{&UntilAwaiter::poll, "a condition"}, pred_(std::move(pred))
    {
    }

    bool await_ready()
    {
        return static_cast<bool>(this->pred_());
    }

    template <typename Promise>
    void await_suspend(std::coroutine_handle<Promise> leaf)
    {
        detail::waitOn(*this, leaf);
    }

    void await_resume() const noexcept
    {
    }

  private:
    static bool poll(detail::SequenceWait* wait, Time)
    {
        return static_cast<bool>(static_cast<UntilAwaiter*>(wait)->pred_());
    }

    Pred pred_;
};

/// @brief `co_await hypha::until(pred)` in a Task: wait until `pred()` holds.
template <typename Pred>
UntilAwaiter<Pred> until(Pred pred)
{
    return UntilAwaiter<Pred>(std::move(pred));
}

}  // namespace hypha
