#pragma once

#include <functional>
#include <memory>
#include <type_traits>
#include <utility>

namespace hypha {

/// A callback that borrows its object. The object must outlive the callback.
template <auto Method, typename Object>
struct MemberCallback
{
    Object* object;

    template <typename... Args>
    std::invoke_result_t<decltype(Method), Object&, Args...> operator()(Args&&... args) const
    {
        return std::invoke(Method, *this->object, std::forward<Args>(args)...);
    }
};

/// Bind a named member function without taking ownership of its object. The object must
/// outlive every registration that stores the callback; do not pass a temporary.
template <auto Method, typename Object>
MemberCallback<Method, Object> bind(Object& object)
{
    return {std::addressof(object)};
}

}  // namespace hypha
