#pragma once

#include "hypha/backend/config_readers.h"

#include <concepts>
#include <cstdint>
#include <format>
#include <stdexcept>
#include <string>
#include <utility>

namespace hypha {

namespace constraints {

struct Number
{
    ::HyphaNumberConstraint kind;
    double                  minimum{0.0};
    double                  maximum{0.0};
    std::string             description;

    template <std::floating_point T>
    bool accepts(T value) const
    {
        return backend::paramValidateNumber(value, this->kind, this->minimum, this->maximum)
            == HYPHA_STATUS_OK;
    }
};

inline Number finite()
{
    return {::HyphaNumberConstraint::HYPHA_NUMBER_CONSTRAINT_FINITE, 0, 0, "must be finite"};
}

inline Number positive()
{
    return {
        ::HyphaNumberConstraint::HYPHA_NUMBER_CONSTRAINT_POSITIVE,
        0,
        0,
        "must be finite and positive"
    };
}

inline Number nonNegative()
{
    return {
        ::HyphaNumberConstraint::HYPHA_NUMBER_CONSTRAINT_NON_NEGATIVE,
        0,
        0,
        "must be finite and non-negative"
    };
}

inline Number negative()
{
    return {
        ::HyphaNumberConstraint::HYPHA_NUMBER_CONSTRAINT_NEGATIVE,
        0,
        0,
        "must be finite and negative"
    };
}

inline Number nonPositive()
{
    return {
        ::HyphaNumberConstraint::HYPHA_NUMBER_CONSTRAINT_NON_POSITIVE,
        0,
        0,
        "must be finite and non-positive"
    };
}

inline Number range(double minimum, double maximum)
{
    return {
        ::HyphaNumberConstraint::HYPHA_NUMBER_CONSTRAINT_RANGE,
        minimum,
        maximum,
        std::format("must be finite and in [{}, {}]", minimum, maximum)
    };
}

/// Inclusive integer bounds, compared exactly whatever the integer width and signedness.
struct IntegerRange
{
    std::int64_t minimum{0};
    std::int64_t maximum{0};
    std::string  description;

    template <std::integral T>
    bool accepts(T value) const
    {
        const bool atLeastMinimum = std::cmp_greater_equal(value, this->minimum);
        const bool atMostMaximum = std::cmp_less_equal(value, this->maximum);
        return atLeastMinimum && atMostMaximum;
    }

    template <std::floating_point T>
    bool accepts(T value) const
    {
        const Number bounds =
            range(static_cast<double>(this->minimum), static_cast<double>(this->maximum));
        return bounds.accepts(value);
    }
};

/// @brief The inclusive range [@p minimum, @p maximum] for an integer parameter:
///        `required<uint8_t>("channels", constraints::range(1, 8))`.
/// @throws std::invalid_argument when a bound does not fit int64_t.
template <std::integral Minimum, std::integral Maximum>
IntegerRange range(Minimum minimum, Maximum maximum)
{
    const bool boundsFit =
        std::in_range<std::int64_t>(minimum) && std::in_range<std::int64_t>(maximum);
    if (!boundsFit) {
        throw std::invalid_argument("hypha: an integer range bound does not fit int64_t");
    }
    return {
        static_cast<std::int64_t>(minimum),
        static_cast<std::int64_t>(maximum),
        std::format("must be in [{}, {}]", minimum, maximum)
    };
}

struct Nonempty
{
    const char* description{"must not be empty"};

    bool accepts(const std::string& value) const
    {
        return backend::paramValidateNonempty(value.data(), value.size()) == HYPHA_STATUS_OK;
    }
};

inline Nonempty nonempty()
{
    return {};
}

}  // namespace constraints

namespace detail {
struct Unconstrained
{
    const char* description{""};

    template <typename T>
    bool accepts(const T&) const
    {
        return true;
    }
};
}  // namespace detail

}  // namespace hypha
