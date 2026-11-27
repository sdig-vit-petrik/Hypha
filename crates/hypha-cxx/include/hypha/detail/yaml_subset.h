#pragma once

/// @file
/// The block YAML that tools write for plain data (the ROS camera calibration is one): nested
/// `key: value` mappings, scalars, and lists as `[a, b]` (on one or several lines) or as
/// `- a` lines. Anchors, tags, multi-line scalars, flow mappings and lists inside lists are
/// refused with the line, not guessed. Internal: camera_info.h is the API.

#include "hypha/status.h"

#include <algorithm>
#include <cstddef>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace hypha::detail::yaml {

struct Node
{
    enum class Kind { Scalar, Sequence, Mapping };

    Kind                     kind{Kind::Scalar};
    size_t                   line{0};  ///< the line of its key, or of the list item
    std::string              text;  ///< a scalar, without quotes
    std::vector<Node>        items;  ///< a list's items, all scalars
    std::vector<std::string> keys;  ///< a mapping's keys, in file order
    std::vector<Node>        values;  ///< one for each key

    const Node* find(std::string_view key) const
    {
        for (size_t index = 0; index < this->keys.size(); ++index) {
            if (this->keys[index] == key) {
                return &this->values[index];
            }
        }
        return nullptr;
    }
};

/// @brief The error of @p where (`camera calibration 'file'`) at @p line (0: the file).
[[noreturn]] inline void fail(const std::string& where, size_t line, const std::string& what)
{
    const std::string position = line == 0 ? "" : "line " + std::to_string(line) + ": ";
    throw ConfigError("hypha: " + where + ": " + position + what);
}

namespace parse_detail {

struct Line
{
    size_t      number{0};
    size_t      indent{0};
    std::string text;  ///< without indentation, comment and trailing blanks
};

inline std::string unquote(std::string text)
{
    const bool quoted = text.size() >= 2 && (text.front() == '\'' || text.front() == '"')
                     && text.back() == text.front();
    return quoted ? text.substr(1, text.size() - 2) : text;
}

inline std::string trim(const std::string& text)
{
    const size_t first = text.find_first_not_of(" \t");
    if (first == std::string::npos) {
        return {};
    }
    return text.substr(first, text.find_last_not_of(" \t") - first + 1);
}

/// A '#' that starts a comment: at the start of a line or after a blank, outside quotes.
inline std::string withoutComment(const std::string& text)
{
    char quote = 0;
    for (size_t index = 0; index < text.size(); ++index) {
        const char c = text[index];
        const bool opens = quote == 0 && (c == '\'' || c == '"')
                        && (index == 0 || text[index - 1] == ' ' || text[index - 1] == '[');
        if (opens) {
            quote = c;
        } else if (c == quote) {
            quote = 0;
        } else if (c == '#' && quote == 0 && (index == 0 || text[index - 1] == ' ')) {
            return text.substr(0, index);
        }
    }
    return text;
}

class Parser
{
  public:
    Parser(std::string_view text, std::string where) : where_(std::move(where))
    {
        size_t number = 0;
        size_t start = 0;
        while (start <= text.size()) {
            const size_t end = std::min(text.find('\n', start), text.size());
            ++number;
            this->addLine(number, std::string(text.substr(start, end - start)));
            start = end + 1;
        }
        const bool documentStart = !this->lines_.empty() && this->lines_[0].text == "---";
        this->pos_ = documentStart ? 1 : 0;
    }

    Node parse()
    {
        if (this->pos_ >= this->lines_.size()) {
            fail(this->where_, 0, "the file is empty");
        }
        Node root = this->mapping(this->lines_[this->pos_].indent);
        if (this->pos_ < this->lines_.size()) {
            fail(this->where_, this->lines_[this->pos_].number, "unexpected indentation");
        }
        return root;
    }

  private:
    void addLine(size_t number, std::string raw)
    {
        if (!raw.empty() && raw.back() == '\r') {
            raw.pop_back();
        }
        const std::string content = trim(withoutComment(raw));
        if (content.empty()) {
            return;
        }
        const size_t indent = raw.find_first_not_of(' ');
        if (raw[indent] == '\t') {
            fail(this->where_, number, "a tab indents the line");
        }
        this->lines_.push_back({number, indent, content});
    }

    static bool isItem(const Line& line)
    {
        return line.text == "-" || line.text.starts_with("- ");
    }

    Node mapping(size_t indent)
    {
        Node map;
        map.kind = Node::Kind::Mapping;
        while (this->pos_ < this->lines_.size() && this->lines_[this->pos_].indent == indent) {
            const Line line = this->lines_[this->pos_++];
            const auto [key, rest] = this->split(line);
            const bool repeated = std::ranges::find(map.keys, key) != map.keys.end();
            if (repeated) {
                fail(this->where_, line.number, "key '" + key + "' is set twice");
            }
            Node value = this->value(line, key, rest, indent);
            value.line = line.number;
            map.keys.push_back(key);
            map.values.push_back(std::move(value));
        }
        const bool deeper =
            this->pos_ < this->lines_.size() && this->lines_[this->pos_].indent > indent;
        if (deeper) {
            fail(this->where_, this->lines_[this->pos_].number, "unexpected indentation");
        }
        return map;
    }

    /// `key: rest`, the key unquoted.
    std::pair<std::string, std::string> split(const Line& line) const
    {
        const std::string& text = line.text;
        size_t             colon = std::string::npos;
        size_t             from = 0;
        if (text.front() == '\'' || text.front() == '"') {
            from = std::max<size_t>(text.find(text.front(), 1), 1);
        }
        for (size_t index = text.find(':', from); index != std::string::npos;
             index = text.find(':', index + 1)) {
            if (index + 1 == text.size() || text[index + 1] == ' ') {
                colon = index;
                break;
            }
        }
        const bool isMapping = colon != std::string::npos && !isItem(line) && text.front() != '[';
        if (!isMapping) {
            fail(this->where_, line.number, "expected 'key: value'");
        }
        return {unquote(trim(text.substr(0, colon))), trim(text.substr(colon + 1))};
    }

    Node value(const Line& line, const std::string& key, const std::string& rest, size_t indent)
    {
        Node node;
        if (rest.empty()) {
            const bool   hasNext = this->pos_ < this->lines_.size();
            const size_t next = hasNext ? this->lines_[this->pos_].indent : 0;
            if (hasNext && next > indent) {
                return isItem(this->lines_[this->pos_]) ? this->sequence(next)
                                                        : this->mapping(next);
            }
            if (hasNext && next == indent && isItem(this->lines_[this->pos_])) {
                return this->sequence(indent);
            }
        } else if (rest.front() == '[') {
            return this->flowSequence(line, key, rest, indent);
        } else if (rest.front() == '{') {
            fail(
                this->where_,
                line.number,
                key
                    + ": a flow mapping '{...}' is not supported; write its entries on their own "
                      "lines"
            );
        }
        node.text = unquote(rest);
        return node;
    }

    Node sequence(size_t indent)
    {
        Node list;
        list.kind = Node::Kind::Sequence;
        while (this->pos_ < this->lines_.size() && this->lines_[this->pos_].indent == indent
               && isItem(this->lines_[this->pos_])) {
            const Line& line = this->lines_[this->pos_++];
            Node        item;
            item.line = line.number;
            item.text = unquote(trim(line.text.substr(1)));
            list.items.push_back(std::move(item));
        }
        const bool deeper =
            this->pos_ < this->lines_.size() && this->lines_[this->pos_].indent > indent;
        if (deeper) {
            fail(this->where_, this->lines_[this->pos_].number, "unexpected indentation");
        }
        return list;
    }

    /// `[a, b, c]`, which may run over lines that are indented deeper than the key.
    Node flowSequence(const Line& first, const std::string& key, std::string rest, size_t indent)
    {
        Node list;
        list.kind = Node::Kind::Sequence;
        size_t line = first.number;
        bool   needsComma = false;
        rest.erase(0, 1);
        for (;;) {
            const size_t      close = rest.find(']');
            const std::string fragment = rest.substr(0, close);
            if (fragment.find('[') != std::string::npos) {
                fail(this->where_, line, key + ": a list inside a list is not supported");
            }
            if (needsComma && !trim(fragment).empty()) {
                fail(this->where_, line, key + ": a ',' is missing between list items");
            }
            needsComma = this->addItems(list, fragment, line, key);
            if (close != std::string::npos) {
                if (!trim(rest.substr(close + 1)).empty()) {
                    fail(this->where_, line, key + ": text after the closing ']'");
                }
                return list;
            }
            const bool continues =
                this->pos_ < this->lines_.size() && this->lines_[this->pos_].indent > indent;
            if (!continues) {
                fail(this->where_, first.number, key + ": the list is not closed");
            }
            line = this->lines_[this->pos_].number;
            rest = this->lines_[this->pos_++].text;
        }
    }

    /// Adds the comma-separated items of @p fragment; true when the last one has no comma.
    bool addItems(Node& list, const std::string& fragment, size_t line, const std::string& key)
    {
        bool   pending = false;
        size_t start = 0;
        while (start <= fragment.size()) {
            const size_t      comma = std::min(fragment.find(',', start), fragment.size());
            const std::string text = trim(fragment.substr(start, comma - start));
            const bool        last = comma == fragment.size();
            if (!text.empty()) {
                Node item;
                item.line = line;
                item.text = unquote(text);
                list.items.push_back(std::move(item));
                pending = last;
            } else if (!last) {
                fail(this->where_, line, key + ": an item of the list is empty");
            }
            start = comma + 1;
        }
        return pending;
    }

    std::string       where_;
    std::vector<Line> lines_;
    size_t            pos_{0};
};

}  // namespace parse_detail

/// @brief The mapping in @p text, a YAML document of the subset above.
/// @throws ConfigError naming @p where and the line of a construct outside the subset.
inline Node parse(std::string_view text, const std::string& where)
{
    return parse_detail::Parser(text, where).parse();
}

}  // namespace hypha::detail::yaml
