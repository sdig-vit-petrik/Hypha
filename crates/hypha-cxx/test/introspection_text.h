#pragma once

// The introspection snapshot of this node as plain lines, for assertions. A
// test aid, not a table format: one line per section header and one indented
// line per row, cells joined by two spaces. An empty cell is `-`, an empty
// configuration value is `""`. The typed facts of an endpoint or a binding are
// one cell of comma-joined tokens, such as `depth=4` or `queued=16`.

#include <hypha/introspection.h>

#include <cstdint>
#include <stdexcept>
#include <string>
#include <vector>

namespace test_support {

inline std::string cellText(const std::string& text)
{
    return text.empty() ? std::string("-") : text;
}

inline std::string valueText(const std::string& text)
{
    return text.empty() ? std::string("\"\"") : text;
}

inline const char* sourceText(uint8_t source)
{
    switch (source) {
        case hypha_msgs::IntrospectedConfig::SOURCE_PARAMETER:
            return "parameter";
        case hypha_msgs::IntrospectedConfig::SOURCE_APPLICATION:
            return "application";
        case hypha_msgs::IntrospectedConfig::SOURCE_COMMAND_LINE:
            return "command line";
        case hypha_msgs::IntrospectedConfig::SOURCE_DEFAULT:
            return "default";
        default:
            return "?";
    }
}

using Row = std::vector<std::string>;

inline void pushSection(std::string& out, const char* header, const std::vector<Row>& rows)
{
    out += header;
    out += '\n';
    if (rows.empty()) {
        out += "  (none)\n";
    }
    for (const Row& row : rows) {
        out += "  ";
        for (size_t index = 0; index < row.size(); ++index) {
            if (index > 0) {
                out += "  ";
            }
            out += row[index];
        }
        out += '\n';
    }
}

/// The typed facts of one endpoint as comma-joined tokens, or `-` when it has none.
inline std::string endpointFacts(const hypha_msgs::IntrospectedEndpoint& endpoint)
{
    std::vector<std::string> facts;
    if (endpoint.queue_depth > 0U) {
        facts.push_back("depth=" + std::to_string(endpoint.queue_depth));
    }
    if (endpoint.latched) {
        facts.push_back("latched");
    }
    if (endpoint.output_failure == hypha_msgs::IntrospectedEndpoint::OUTPUT_FAIL_RUN) {
        facts.push_back("output=" + endpoint.output_cycle + "/fail_run");
    }
    if (endpoint.output_failure == hypha_msgs::IntrospectedEndpoint::OUTPUT_DROP_POOL_EXHAUSTED) {
        facts.push_back("output=" + endpoint.output_cycle + "/drop_pool_exhausted");
    }
    if (facts.empty()) {
        return "-";
    }
    std::string joined;
    for (const std::string& fact : facts) {
        if (!joined.empty()) {
            joined += ',';
        }
        joined += fact;
    }
    return joined;
}

/// The capture facts of one binding as comma-joined tokens, such as
/// `queued=16,silence=50000000,stagnation=0,counted`.
inline std::string captureFacts(const hypha_msgs::IntrospectedInput& input)
{
    std::string facts;
    if (input.mode == hypha_msgs::IntrospectedInput::MODE_QUEUED) {
        facts = "queued=" + std::to_string(input.max_samples_per_iteration);
    } else {
        facts = "latest";
    }
    if (input.supervised) {
        facts += ",silence=" + std::to_string(input.silence_nanos);
        facts += ",stagnation=" + std::to_string(input.stagnation_nanos);
        facts += input.ignored ? ",ignored" : ",counted";
    }
    return facts;
}

inline std::vector<Row> endpointRows(const std::vector<hypha_msgs::IntrospectedEndpoint>& endpoints)
{
    std::vector<Row> rows;
    for (const hypha_msgs::IntrospectedEndpoint& endpoint : endpoints) {
        rows.push_back(
            {cellText(endpoint.topic), cellText(endpoint.data_type), endpointFacts(endpoint)}
        );
    }
    return rows;
}

inline std::string dump(const hypha_msgs::NodeIntrospection& snapshot)
{
    std::string      out = "hypha node " + cellText(snapshot.node) + "\n";
    std::vector<Row> configuration;
    for (const hypha_msgs::IntrospectedConfig& row : snapshot.configuration) {
        const std::string defaultText = row.has_default ? valueText(row.default_value) : "-";
        configuration.push_back(
            {cellText(row.key), valueText(row.value), defaultText, sourceText(row.source)}
        );
    }
    pushSection(out, "configuration  value  default  source", configuration);
    pushSection(out, "subscribers  type  facts", endpointRows(snapshot.subscribers));
    pushSection(out, "publishers  type  facts", endpointRows(snapshot.publishers));
    std::vector<Row> cycles;
    for (const hypha_msgs::IntrospectedCycle& cycle : snapshot.cycles) {
        std::string rate;
        const bool  hasRate = cycle.has_period && cycle.period_nanos > 0;
        if (hasRate) {
            const uint64_t tenths = 10'000'000'000ULL / cycle.period_nanos;
            rate = std::to_string(tenths / 10) + "." + std::to_string(tenths % 10) + " Hz";
        }
        cycles.push_back(
            {cellText(cycle.name),
             cellText(rate),
             std::to_string(cycle.inputs),
             std::to_string(cycle.outputs)}
        );
    }
    pushSection(out, "cycles  rate  inputs  outputs", cycles);
    if (!snapshot.input_bindings.empty()) {
        std::vector<Row> bindings;
        for (const hypha_msgs::IntrospectedInput& input : snapshot.input_bindings) {
            bindings.push_back(
                {cellText(input.cycle),
                 std::to_string(input.index),
                 cellText(input.topic),
                 input.required ? "required" : "optional"}
            );
        }
        pushSection(out, "cycle  input  topic  role", bindings);
        std::vector<Row> captures;
        for (const hypha_msgs::IntrospectedInput& input : snapshot.input_bindings) {
            captures.push_back(
                {cellText(input.cycle), std::to_string(input.index), captureFacts(input)}
            );
        }
        pushSection(out, "cycle  input  capture", captures);
    }
    return out;
}

/// The snapshot of this node as plain lines.
/// @throws std::runtime_error when the snapshot is refused.
inline std::string introspectionText()
{
    const std::expected<hypha_msgs::NodeIntrospection, hypha::Status> snapshot =
        hypha::introspect();
    if (!snapshot) {
        throw std::runtime_error(
            "introspect() failed: status " + std::to_string(snapshot.error().asInt())
        );
    }
    return dump(*snapshot);
}

}  // namespace test_support
