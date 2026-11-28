#pragma once

/// @file
/// hypha::Sequence: a chain of calls in a cycle, written in the order it runs, that ends in
/// the answer to a deferred request (docs/design/native-services.md §8.2).
///
///     hypha::Task<std_srvs::Trigger::Response> runTakeoff() {
///         auto a = co_await hypha::call(this->switchController, {.value = "mpc"});
///         if (!succeeded(a)) co_return failure("could not switch the controller", a);
///         co_await hypha::sleepFor(hypha::Duration::fromMillis(300));
///         ...
///         co_return {.success = true, .message = "taking off"};
///     }
///     // in the step
///     for (auto& request : this->takeoff.requests())
///         this->takeoffSeq_.start(request.defer(), this->runTakeoff());
///     this->takeoffSeq_.advance(time);
///
/// A Task is a C++ coroutine that only Sequence::advance() resumes, in the step, on the run
/// thread: `co_await hypha::call(port, request)` stages the call as port.call() does and
/// resumes with its CallResult in the first step that reads the outcome; a call that
/// returns Busy or TooLarge resumes at once, NotSent with that status. `co_await
/// hypha::sleepFor(d)` resumes at the first advance at least `d` of step time later, and
/// `co_await hypha::until(pred)` once `pred()` holds, checked when reached and at each
/// advance. A Task may `co_await` another Task. advance() runs the sequence until it waits
/// for something not ready, so what it reads and calls follows the step's inputs alone,
/// and a replay repeats it.
///
/// `co_return` answers the Deferred given to start(); a Task that throws, or an advance()
/// that throws, ends the sequence, refuses that request with the reason and rethrows, so
/// the step fails (its client then reads ServerLost). Destroying a sequence destroys its
/// frames, and its Deferred refuses. One heap frame is allocated per Task called; an
/// advance() allocates nothing of its own.
///
/// The rules of a step hold: call advance() in every step while the sequence runs, since
/// a step that does not read an outcome drops it (advance() then throws std::logic_error,
/// naming the port); read no other outcome of a port the sequence waits on; hold no
/// reference to step-local data or to an input's value across a `co_await`.

#include "hypha/cycle.h"
#include "hypha/sequence_task.h"
#include "hypha/service.h"

#include <exception>
#include <optional>
#include <stdexcept>
#include <string>
#include <utility>

namespace hypha::detail {

/// What a Sequence of either kind does with its Task.
template <typename T>
class SequenceRunner
{
  public:
    SequenceRunner(const SequenceRunner&) = delete;
    SequenceRunner& operator=(const SequenceRunner&) = delete;
    SequenceRunner(SequenceRunner&&) = delete;
    SequenceRunner& operator=(SequenceRunner&&) = delete;

    /// True from start() until the Task ends.
    bool running() const
    {
        return static_cast<bool>(this->task_.frame_);
    }

    /// What the sequence waits on now: `the outcome of a call to 'arming'`, `the end of a
    /// sleep`, `a condition`, `its first advance`; empty when it does not run.
    std::string waitingFor() const
    {
        if (!this->running()) {
            return {};
        }
        if (!this->started_) {
            return "its first advance";
        }
        const SequenceWait* wait = this->core_.waiting;
        if (wait == nullptr) {
            return {};
        }
        return wait->port == nullptr ? std::string(wait->what)
                                     : std::string(wait->what) + " '" + *wait->port + "'";
    }

  protected:
    SequenceRunner() = default;
    ~SequenceRunner() = default;

    void begin(Task<T> task)
    {
        if (this->running()) {
            throw std::logic_error("hypha::Sequence::start() while it runs (running())");
        }
        if (!task.frame_) {
            throw std::logic_error("hypha::Sequence::start() with an empty Task");
        }
        this->task_ = std::move(task);
        this->started_ = false;
        this->core_ = SequenceCore{};
        this->task_.frame_.promise().core = &this->core_;
    }

    /// Run the Task until it waits on something not ready; its value once it ended, which
    /// ends the sequence. Rethrows what the Task threw, or what a wait threw, and then the
    /// sequence has ended.
    std::optional<TaskValue<T>> run(const CycleTime& time)
    {
        if (!this->running()) {
            return std::nullopt;
        }
        this->core_.now = time.now;
        auto& promise = this->task_.frame_.promise();
        try {
            if (!this->started_) {
                this->started_ = true;
                this->task_.frame_.resume();
            }
            while (!this->task_.frame_.done() && this->core_.waiting != nullptr
                   && this->core_.waiting->ready(this->core_.waiting, time.now)) {
                this->core_.waiting = nullptr;
                this->core_.leaf.resume();
            }
        } catch (...) {
            this->stop();
            throw;
        }
        if (!this->task_.frame_.done()) {
            return std::nullopt;
        }
        const std::exception_ptr    error = promise.error;
        std::optional<TaskValue<T>> value = std::move(promise.value);
        this->stop();
        if (error) {
            std::rethrow_exception(error);
        }
        return value;
    }

    /// Destroy the Task's frames.
    void stop()
    {
        this->task_ = Task<T>{};
        this->core_ = SequenceCore{};
        this->started_ = false;
    }

    /// The reason a request is refused with when the sequence failed with @p error.
    static std::string failureOf(const std::exception_ptr& error)
    {
        try {
            std::rethrow_exception(error);
        } catch (const std::exception& thrown) {
            return std::string("the sequence failed: ") + thrown.what();
        } catch (...) {
            return "the sequence failed";
        }
    }

  private:
    Task<T>      task_{};
    SequenceCore core_{};
    bool         started_{false};
};

}  // namespace hypha::detail

namespace hypha {

/// @brief Runs a Task that answers a request of @p Srv, deferred by a step.
template <typename Srv = void>
class Sequence final : public detail::SequenceRunner<typename Srv::Response>
{
  public:
    using Response = typename Srv::Response;

    Sequence() = default;

    /// @brief Run @p task in the steps that follow; its `co_return` answers @p request.
    ///        It begins at the next advance().
    /// @throws std::logic_error while it runs, or for an empty @p request or @p task.
    void start(Deferred<Srv> request, Task<Response> task)
    {
        if (!request) {
            throw std::logic_error("hypha::Sequence::start() with an empty Deferred");
        }
        this->begin(std::move(task));
        this->request_ = std::move(request);
    }

    /// @brief Run the sequence in the step at @p time: call it in every step while it runs.
    /// @throws what the Task threw, after the request is refused with the reason.
    void advance(const CycleTime& time)
    {
        std::optional<Response> response;
        try {
            response = this->run(time);
        } catch (...) {
            this->answered_ = this->request_.refuse(this->failureOf(std::current_exception()));
            throw;
        }
        if (response.has_value()) {
            this->answered_ = this->request_.reply(*response);
        }
    }

    /// What the last answer to a request returned: Ok, TooLarge (its client was refused
    /// with the sizes) or Disconnected (its client or the server is gone).
    Status lastAnswer() const
    {
        return this->answered_;
    }

  private:
    Deferred<Srv> request_;
    Status        answered_{Status::Ok};
};

/// @brief Runs a Task that answers nothing, such as the work after a request was answered.
template <>
class Sequence<void> final : public detail::SequenceRunner<void>
{
  public:
    Sequence() = default;

    /// @brief Run @p task in the steps that follow, from the next advance().
    /// @throws std::logic_error while it runs, or for an empty @p task.
    void start(Task<void> task)
    {
        this->begin(std::move(task));
    }

    /// @brief Run the sequence in the step at @p time: call it in every step while it runs.
    /// @throws what the Task threw; the sequence has then ended.
    void advance(const CycleTime& time)
    {
        static_cast<void>(this->run(time));
    }
};

}  // namespace hypha
