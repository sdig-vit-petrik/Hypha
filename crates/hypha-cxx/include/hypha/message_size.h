#pragma once

/// @file
/// The serialized size of a message: the number a payload bound is made of.
///
/// `maxPayloadDynamic(bytes)` and a policy's `maxPayloadDynamic` count serialized bytes, the
/// whole message and not its large field. A message of a few fields and one array, an `Image`
/// or a `PointCloud2`, is the size of the array plus a few dozen bytes of the other fields,
/// which these functions count:
///
///     sensor_msgs::Image head;                       // frame_id, encoding: what the node sends
///     head.header.frame_id = "camera";
///     head.encoding = "mono8";
///     const size_t bound = hypha::serializedSize(head, 2592 * 1944);   // 5,038,896: 48 bytes of
///     fields
///
/// To size the pool a bound reserves, run `hypha topic pool --payload <bytes>` (docs/cpp-api.md
/// section 7), or read the line a publisher logs at start.

#include "hypha/backend/ros1_codec.h"
#include "hypha/loan.h"

#include <cstddef>
#include <stdexcept>
#include <string>

namespace hypha {

/// @brief The serialized size of @p message, in bytes: what its publisher puts in a slot.
template <typename M>
size_t serializedSize(const M& message)
{
    return codec::encodedLength(message);
}

/// @brief The serialized size of @p head with its loaned field (its single top-level array of
///        numbers, see Loan) replaced by @p loanedBytes bytes: the bound of a message whose
///        array holds that many bytes, found without allocating them. The array of @p head is
///        not read, so it may be empty.
/// @throws std::invalid_argument when @p loanedBytes is not a whole number of elements.
template <LoanableMessage M>
size_t serializedSize(const M& head, size_t loanedBytes)
{
    using Element = typename detail::LoanedFieldType<M>::value_type;
    if (loanedBytes % sizeof(Element) != 0) {
        throw std::invalid_argument(
            "hypha::serializedSize: " + std::to_string(loanedBytes)
            + " bytes are not a whole number of " + std::to_string(sizeof(Element))
            + "-byte elements"
        );
    }
    const auto&  field = head.*message_traits::LoanedField<M>::member;
    const size_t current = field.size() * sizeof(Element);
    return codec::encodedLength(head) - current + loanedBytes;
}

}  // namespace hypha
