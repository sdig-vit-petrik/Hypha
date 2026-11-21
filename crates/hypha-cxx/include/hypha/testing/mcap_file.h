#pragma once

/// @file
/// detail::McapFile: one open MCAP file of the test-only reader, for testing::McapReplay.

#include <hypha/testing/mcap_reader.h>

#include <cstddef>
#include <cstdint>
#include <stdexcept>
#include <string>

namespace hypha::detail {

/// @brief The text of bytes borrowed from a reader.
inline std::string mcapText(HyphaMcapBytes bytes)
{
    if (bytes.len == 0) {
        return std::string{};
    }
    return std::string(reinterpret_cast<const char*>(bytes.data), bytes.len);
}

/// @brief One open MCAP file. Every refusal of the reader throws std::runtime_error with
///        the reader's text.
class McapFile final
{
  public:
    /// @throws std::runtime_error when the file does not open.
    explicit McapFile(const std::string& path)
    {
        const uint8_t* const  pathBytes = reinterpret_cast<const uint8_t*>(path.data());
        size_t                errorLength = 0;
        const HyphaMcapStatus opened =
            hypha_mcap_open(pathBytes, path.size(), &this->reader_, nullptr, 0, &errorLength);
        if (opened == HYPHA_MCAP_STATUS_OK) {
            return;
        }
        // The first call gave the length of the error text.
        std::string           error(errorLength, '\0');
        size_t                textLength = 0;
        HyphaMcapReader*      retried = nullptr;
        const HyphaMcapStatus again = hypha_mcap_open(
            pathBytes,
            path.size(),
            &retried,
            reinterpret_cast<uint8_t*>(error.data()),
            error.size(),
            &textLength
        );
        if (again == HYPHA_MCAP_STATUS_OK) {
            this->reader_ = retried;
            return;
        }
        const bool textComplete = textLength == error.size();
        if (!textComplete) {
            error = "MCAP file '" + path + "' does not open";
        }
        throw std::runtime_error("hypha: McapReplay: " + error);
    }

    McapFile(const McapFile&) = delete;
    McapFile& operator=(const McapFile&) = delete;
    McapFile(McapFile&&) = delete;
    McapFile& operator=(McapFile&&) = delete;

    ~McapFile()
    {
        hypha_mcap_close(this->reader_);
    }

    size_t channelCount() const
    {
        return hypha_mcap_channel_count(this->reader_);
    }

    /// @brief Channel @p index, 0 <= index < channelCount(); its texts live as long as
    ///        this file.
    HyphaMcapChannel channel(size_t index) const
    {
        HyphaMcapChannel      channel{};
        const HyphaMcapStatus status = hypha_mcap_channel(this->reader_, index, &channel);
        if (status != HYPHA_MCAP_STATUS_OK) {
            this->fail();
        }
        return channel;
    }

    /// @brief Add the channels of @p topic to the messages next() returns.
    void select(const std::string& topic)
    {
        const uint8_t* const  topicBytes = reinterpret_cast<const uint8_t*>(topic.data());
        const HyphaMcapStatus status = hypha_mcap_select(this->reader_, topicBytes, topic.size());
        if (status != HYPHA_MCAP_STATUS_OK) {
            this->fail();
        }
    }

    /// @brief Back to the first message in log-time order.
    void rewind()
    {
        const HyphaMcapStatus status = hypha_mcap_rewind(this->reader_);
        if (status != HYPHA_MCAP_STATUS_OK) {
            this->fail();
        }
    }

    /// @brief The next selected message into @p out; false when none is left. Its payload
    ///        is valid until the next call of next() or rewind().
    bool next(HyphaMcapMessage& out)
    {
        const HyphaMcapStatus status = hypha_mcap_next(this->reader_, &out);
        if (status == HYPHA_MCAP_STATUS_END) {
            return false;
        }
        if (status != HYPHA_MCAP_STATUS_OK) {
            this->fail();
        }
        return true;
    }

  private:
    [[noreturn]] void fail() const
    {
        throw std::runtime_error(
            "hypha: McapReplay: " + mcapText(hypha_mcap_last_error(this->reader_))
        );
    }

    HyphaMcapReader* reader_{nullptr};
};

}  // namespace hypha::detail
