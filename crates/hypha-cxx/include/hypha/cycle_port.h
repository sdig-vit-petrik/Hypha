#pragma once

/// @file
/// Port, the base of every cycle port, and the runner that drives the ports and the step.

#include "hypha/backend/callbacks.h"
#include "hypha/backend/text_readers.h"
#include "hypha/cycle.h"
#include "hypha/detail/report.h"
#include "hypha/options.h"
#include "hypha/topic.h"

#include <cstdint>
#include <exception>
#include <expected>
#include <memory>
#include <new>
#include <optional>
#include <string>
#include <utility>
#include <vector>

namespace hypha {

class Cycle;
class Port;

namespace testing {
class StepHarness;
class McapReplay;
}  // namespace testing

namespace detail {

/// @brief Where a testing::StepHarness step keeps what each output sends.
class CaptureStore
{
  public:
    virtual void* storeFor(const Port& port, void* (*make)(), void (*destroy)(void*)) = 0;

  protected:
    ~CaptureStore() = default;
};

template <typename Step>
class CycleRunner;
struct StepLeave;
struct StepEmission;

/// @brief A failure of the step with the status the run reports. An output throws it
///        when a send is refused before the message reaches the library.
struct StepFailure : std::runtime_error
{
    StepFailure(::HyphaStatus code, const std::string& what)
        : std::runtime_error(what), status(code)
    {
    }

    ::HyphaStatus status;
};

}  // namespace detail

/// @brief The base of every cycle port: Latest, Arrivals, Output and Transforms.
///
/// A port registers with its cycle by address, so it is neither copied nor moved.
class Port
{
  public:
    Port(const Port&) = delete;
    Port& operator=(const Port&) = delete;
    Port(Port&&) = delete;
    Port& operator=(Port&&) = delete;
    virtual ~Port();

  protected:
    explicit Port(Cycle& owner);

    virtual ::HyphaStatus bind(HyphaCycle* cycle) = 0;
    virtual ::HyphaStatus capture(const HyphaCycleImage* image) = 0;
    virtual void          endStep() = 0;

    /// @brief Called once after each step, also when the step throws. Default: nothing.
    virtual void leaveStep()
    {
    }

    virtual std::string describeFailure() const = 0;

    /// @brief False for an input that counts for health and is not usable now
    ///        (Cycle::inputsOk()). Default: true.
    virtual bool inputOk() const
    {
        return true;
    }

    /// @brief True when the last bind() was refused by the port's table or tags.
    ///        Default: false.
    virtual bool bindRefusedByConfiguration() const
    {
        return false;
    }

    /// @brief The binding index of a port that can trigger its cycle, once bound.
    ///        Default: none.
    virtual std::optional<uint32_t> triggerIndex() const
    {
        return std::nullopt;
    }

    ::HyphaStatus bindInputSource(
        HyphaCycle*                  cycle,
        HyphaInputSource*            source,
        bool                         queued,
        const detail::InputSettings& settings,
        uint32_t&                    index
    ) const;

    /// @brief The cycle handle while the owner is live, else nullptr.
    HyphaCycle* liveCycle() const;

    /// @brief The current callback emission handle, or nullptr outside the user step.
    HyphaCycleEmission* liveEmission() const
    {
        return this->emission_;
    }

    /// @brief The store of a testing::StepHarness step, or nullptr outside one.
    detail::CaptureStore* harnessStore() const
    {
        return this->harness_;
    }

    /// @brief Record @p status as this step's failure and throw it. The failure stands
    ///        also when the step catches the exception.
    [[noreturn]] void failStep(::HyphaStatus status, const std::string& what)
    {
        if (!this->stepFailure_.has_value()) {
            this->stepFailure_ = status;
        }
        throw detail::StepFailure(status, what);
    }

    /// @brief Refuse a configuration call once the cycle has started.
    void refuseAfterStart(const char* call) const;

    /// @brief True once the owning cycle has started.
    bool started() const;

  private:
    friend class Cycle;
    template <typename Step>
    friend class detail::CycleRunner;
    friend struct detail::StepLeave;
    friend struct detail::StepEmission;
    friend class testing::StepHarness;

    void setEmission(HyphaCycleEmission* emission)
    {
        this->emission_ = emission;
    }

    std::optional<::HyphaStatus> takeStepFailure()
    {
        return std::exchange(this->stepFailure_, std::nullopt);
    }

    Cycle&                       owner_;
    HyphaCycleEmission*          emission_{nullptr};
    detail::CaptureStore*        harness_{nullptr};
    std::optional<::HyphaStatus> stepFailure_;
};

namespace detail {

/// @brief The text of a failed start().
inline std::string cycleFailure(const std::string& cycleName, const std::string& where, int status)
{
    std::string text = "hypha: cycle '" + cycleName + "': ";
    if (!where.empty()) {
        text += where + ": ";
    }
    text += Status(static_cast<::HyphaStatus>(status)).name();
    const std::string reason = backend::lastError();
    if (!reason.empty()) {
        text += ": " + reason;
    }
    return text;
}

inline std::string describePort(const char* kind, const TopicSource& topic)
{
    if (topic.fromParam()) {
        return std::string(kind) + " '" + topic.paramName + "'";
    }
    return std::string(kind) + " source";
}

/// @brief The ports one runner may touch. release() clears it, so no tick reaches a port
///        after the cycle was released, even when the C ABI refuses the destroy.
struct CycleLink
{
    std::string        name;
    std::vector<Port*> ports;
    bool               released{false};
};

/// @brief Calls leaveStep() on every port when it goes out of scope.
struct StepLeave
{
    const std::vector<Port*>& ports;

    ~StepLeave()
    {
        for (Port* port : this->ports) {
            port->leaveStep();
        }
    }
};

/// @brief Makes the emission collection available only during the user step.
struct StepEmission
{
    const std::vector<Port*>& ports;

    StepEmission(const std::vector<Port*>& selectedPorts, HyphaCycleEmission* emission)
        : ports(selectedPorts)
    {
        for (Port* port : this->ports) {
            port->setEmission(emission);
        }
    }

    StepEmission(const StepEmission&) = delete;
    StepEmission& operator=(const StepEmission&) = delete;

    ~StepEmission()
    {
        for (Port* port : this->ports) {
            port->setEmission(nullptr);
        }
    }
};

/// @brief Runs the ports and the step for one registered cycle on every tick.
template <typename Step>
class CycleRunner
{
  public:
    CycleRunner(Step step, std::shared_ptr<CycleLink> link)
        : step_(std::move(step)), link_(std::move(link))
    {
    }

    static HyphaCallbackResult
    run(const HyphaCycleImage* image,
        const HyphaCycleTime*  time,
        HyphaCycleEmission*    emission,
        void*                  userData) noexcept
    {
        const bool missingContext = userData == nullptr;
        const bool missingImage = image == nullptr;
        const bool missingTime = time == nullptr;
        const bool missingEmission = emission == nullptr;
        const bool invalidArguments =
            missingContext || missingImage || missingTime || missingEmission;
        if (invalidArguments) {
            return backend::ffi::failRun(HYPHA_STATUS_INVALID_ARGUMENT);
        }
        try {
            return static_cast<CycleRunner*>(userData)->invoke(image, *time, emission);
        } catch (const StepFailure& failure) {
            return backend::ffi::failRun(failure.status);
        } catch (const std::bad_alloc&) {
            return backend::ffi::failRun(HYPHA_STATUS_RESOURCE_LIMIT);
        } catch (...) {
            return backend::ffi::failRun(HYPHA_STATUS_PANIC);
        }
    }

    HyphaCallbackResult invoke(
        const HyphaCycleImage* image, const HyphaCycleTime& rawTime, HyphaCycleEmission* emission
    )
    {
        if (this->link_->released) {
            return backend::ffi::failRun(HYPHA_STATUS_INVALID_STATE);
        }
        const StepLeave leave{this->link_->ports};
        for (Port* port : this->link_->ports) {
            port->endStep();
        }
        const std::expected<CycleTime, ::HyphaStatus> time = cycleTimeFrom(rawTime);
        if (!time) {
            return backend::ffi::failRun(time.error());
        }
        for (Port* port : this->link_->ports) {
            const ::HyphaStatus status = port->capture(image);
            if (status != HYPHA_STATUS_OK) {
                return backend::ffi::failRun(status);
            }
        }
        {
            const StepEmission emissionScope{this->link_->ports, emission};
            this->invokeStep(*time);
        }
        // A send the step refused and caught still fails the step.
        for (Port* port : this->link_->ports) {
            if (const std::optional<::HyphaStatus> failure = port->takeStepFailure()) {
                return backend::ffi::failRun(*failure);
            }
        }
        return backend::ffi::continueRun();
    }

  private:
    /// @brief Call the step; an exception is logged, then rethrown.
    void invokeStep(const CycleTime& time)
    {
        try {
            this->step_(time);
        } catch (const std::exception& error) {
            detail::error("hypha: cycle '" + this->link_->name + "': step threw: " + error.what());
            throw;
        } catch (...) {
            detail::error(
                "hypha: cycle '" + this->link_->name
                + "': step threw an exception that is not a std::exception"
            );
            throw;
        }
    }

    Step                       step_;
    std::shared_ptr<CycleLink> link_;
};

}  // namespace detail

}  // namespace hypha
