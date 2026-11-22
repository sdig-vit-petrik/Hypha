#pragma once

/// @file
/// RCLCPP_COMPONENTS_REGISTER_NODE registers a node class with the shim's component
/// runner (shim/src/component_main.cpp), which builds one process per component, as
/// rclcpp_components_register_node(... EXECUTABLE ...) does.

#include <rclcpp/rclcpp.hpp>

#include <functional>
#include <memory>
#include <string>
#include <vector>

namespace rclcpp_components {

using NodeFactory = std::function<std::shared_ptr<rclcpp::Node>(const rclcpp::NodeOptions&)>;

struct Component
{
    std::string className;
    NodeFactory factory;
};

/// The components linked into this program, in registration order.
std::vector<Component>& registry();

struct Registrar
{
    Registrar(const char* className, NodeFactory factory)
    {
        registry().push_back(Component{className, std::move(factory)});
    }
};

}  // namespace rclcpp_components

#define HYPHA_RCLCPP_CAT_INNER(a, b) a##b
#define HYPHA_RCLCPP_CAT(a, b) HYPHA_RCLCPP_CAT_INNER(a, b)

#define RCLCPP_COMPONENTS_REGISTER_NODE(NodeClass)                                                 \
    namespace {                                                                                    \
    const ::rclcpp_components::Registrar HYPHA_RCLCPP_CAT(hyphaRclcppComponent, __LINE__){         \
        #NodeClass, [](const ::rclcpp::NodeOptions& options) {                                     \
            return std::static_pointer_cast<::rclcpp::Node>(std::make_shared<NodeClass>(options)); \
        }                                                                                          \
    };                                                                                             \
    }
