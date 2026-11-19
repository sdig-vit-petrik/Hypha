#pragma once

/// @file
/// hypha::visitArrivalsInOrder(): this step's samples of several Arrivals ports in stamp
/// order.

#include "hypha/cycle_arrivals.h"

#include <compare>
#include <cstddef>
#include <optional>
#include <span>
#include <tuple>
#include <type_traits>
#include <utility>

namespace hypha {

namespace detail {

/// The merge key of one sample: its stamp, then its port, then its place in the batch.
template <typename Stamp>
struct MergeKey
{
    Stamp  stamp;
    size_t port;
    size_t sample;

    bool operator<(const MergeKey& other) const
    {
        using Order = std::compare_three_way_result_t<Stamp>;
        const Order order = this->stamp <=> other.stamp;
        if (order != 0) {
            return order < 0;
        }
        if (this->port != other.port) {
            return this->port < other.port;
        }
        return this->sample < other.sample;
    }
};

template <typename StampFn, typename M>
using StampOf = std::remove_cvref_t<std::invoke_result_t<StampFn&, const M&>>;

template <size_t I, typename Key, typename Batches, typename StampFn>
void offerEarliest(
    const Batches&            batches,
    StampFn&                  stampOf,
    const std::optional<Key>& last,
    std::optional<Key>&       earliest
)
{
    using Batch = std::tuple_element_t<I, Batches>;
    const Batch& batch = std::get<I>(batches);
    for (size_t sample = 0; sample < batch.size(); ++sample) {
        Key        candidate{std::invoke(stampOf, batch[sample]), I, sample};
        const bool unvisited = !last || *last < candidate;
        const bool earlier = !earliest || candidate < *earliest;
        const bool takes = unvisited && earlier;
        if (takes) {
            earliest.emplace(std::move(candidate));
        }
    }
}

template <typename Key, typename Batches, typename StampFn, size_t... I>
std::optional<Key> findEarliest(
    const Batches&            batches,
    StampFn&                  stampOf,
    const std::optional<Key>& last,
    std::index_sequence<I...>
)
{
    std::optional<Key> earliest;
    (offerEarliest<I, Key>(batches, stampOf, last, earliest), ...);
    return earliest;
}

template <size_t I, typename Key, typename Batches, typename Visitor>
void visitIfFrom(const Batches& batches, Visitor& visit, const Key& earliest)
{
    if (earliest.port == I) {
        std::invoke(
            visit, std::integral_constant<size_t, I>{}, std::get<I>(batches)[earliest.sample]
        );
    }
}

template <typename Key, typename Batches, typename Visitor, size_t... I>
void visitEarliest(
    const Batches& batches, Visitor& visit, const Key& earliest, std::index_sequence<I...>
)
{
    (visitIfFrom<I>(batches, visit, earliest), ...);
}

}  // namespace detail

/// @brief Visit this step's samples of several Arrivals ports in ascending stamp order,
///        without allocating.
///
/// @p stampOf returns a totally ordered stamp of one type for every port. @p visit takes
/// the port position as `std::integral_constant<size_t, I>` and the `const M&`. Equal
/// stamps keep the argument order of the ports, then the batch order.
///
/// It calls @p stampOf O(N^2) times for N samples, so keep it cheap.
template <typename StampFn, typename Visitor, typename... Messages>
void visitArrivalsInOrder(StampFn&& stampOf, Visitor&& visit, const Arrivals<Messages>&... ports)
{
    static_assert(sizeof...(Messages) > 0, "visitArrivalsInOrder() needs at least one port");
    using Stamp = std::common_type_t<detail::StampOf<StampFn, Messages>...>;
    static_assert(
        (std::is_same_v<Stamp, detail::StampOf<StampFn, Messages>> && ...),
        "stampOf must return the same stamp type for every port"
    );
    static_assert(
        std::three_way_comparable<Stamp, std::weak_ordering>,
        "the stamp needs a total order; use integer nanoseconds or hypha::Time"
    );
    using Key = detail::MergeKey<Stamp>;
    using Ports = std::index_sequence_for<Messages...>;
    using Batches = std::tuple<std::span<const Messages>...>;

    const Batches      batches{ports.samples()...};
    const size_t       total = (ports.samples().size() + ...);
    std::optional<Key> last;
    for (size_t visited = 0; visited < total; ++visited) {
        std::optional<Key> earliest = detail::findEarliest<Key>(batches, stampOf, last, Ports{});
        // Keys are distinct and totally ordered, so an unvisited sample always remains.
        detail::visitEarliest(batches, visit, *earliest, Ports{});
        last = std::move(earliest);
    }
}

}  // namespace hypha
