#pragma once

#include <cstddef>
#include <functional>
#include <memory>
#include <tuple>
#include <type_traits>
#include <utility>

namespace hypha {

/// A callback that borrows its object and holds the arguments bound with it. The object
/// must outlive the callback.
template <auto Method, typename Object, typename... Bound>
struct MemberCallback
{
    Object*              object;
    std::tuple<Bound...> bound;

    template <typename... Args>
    std::invoke_result_t<decltype(Method), Object&, const Bound&..., Args...>
    operator()(Args&&... args) const
    {
        return this->invoke(std::index_sequence_for<Bound...>{}, std::forward<Args>(args)...);
    }

  private:
    template <std::size_t... Indices, typename... Args>
    std::invoke_result_t<decltype(Method), Object&, const Bound&..., Args...>
    invoke(std::index_sequence<Indices...>, Args&&... args) const
    {
        return std::invoke(
            Method, *this->object, std::get<Indices>(this->bound)..., std::forward<Args>(args)...
        );
    }
};

/// Bind a named member function without taking ownership of its object. The object must
/// outlive every registration that stores the callback; do not pass a temporary.
///
/// Arguments after the object are bound: copied into the callback and passed to the member
/// before the arguments of the call, as `std::bind_front` does, as const lvalues. A member
/// that must know which of several inputs a sample came from takes its index first:
/// `node.subscribe<M>(topic, hypha::bind<&Mux::onInput>(*this, index))` calls
/// `onInput(index, message)`. A bound pointer must outlive the callback as well.
template <auto Method, typename Object, typename... Bound>
MemberCallback<Method, Object, std::decay_t<Bound>...> bind(Object& object, Bound&&... bound)
{
    return {
        std::addressof(object), std::tuple<std::decay_t<Bound>...>(std::forward<Bound>(bound)...)
    };
}

}  // namespace hypha
