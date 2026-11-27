// The typed loan's contract (#255): wire identity for every loanable type, refusals that
// send nothing and take no sequence, lifetime, integrity, alignment. Written as an
// adversarial review of the first version; each test names the behaviour it holds.
#include "test_types.h"

#include <gtest/gtest.h>
#include <nav_msgs/OccupancyGrid.h>
#include <sensor_msgs/CompressedImage.h>
#include <sensor_msgs/Image.h>
#include <sensor_msgs/PointCloud2.h>
#include <std_msgs/ByteMultiArray.h>
#include <std_msgs/Float32MultiArray.h>
#include <std_msgs/Float64MultiArray.h>
#include <std_msgs/Int16MultiArray.h>
#include <std_msgs/Int32MultiArray.h>
#include <std_msgs/Int64MultiArray.h>
#include <std_msgs/Int8MultiArray.h>
#include <std_msgs/UInt16MultiArray.h>
#include <std_msgs/UInt32MultiArray.h>
#include <std_msgs/UInt64MultiArray.h>
#include <std_msgs/UInt8MultiArray.h>

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <optional>
#include <stdexcept>
#include <thread>
#include <vector>

using hypha_test::testTopic;

namespace {

class TypedLoanContract : public ::testing::Test
{
  public:
    void SetUp() override
    {
        ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1000000000)));
        this->node.emplace("typed_loan_contract");
    }

    template <typename M>
    hypha::RawSubscriber collect(const std::string& topic, std::vector<std::vector<uint8_t>>& into)
    {
        return hypha::subscribeRawOfType(
                   topic,
                   hypha::codec::dataType<M>(),
                   [&into](const hypha::RawSample& sample) {
                       into.emplace_back(sample.bytes.begin(), sample.bytes.end());
                   },
                   0,
                   hypha::defaultQos
        )
            .value();
    }

    void deliver()
    {
        (void)this->node->runFor(hypha::Duration::fromMillis(5));
    }

    std::optional<hypha::Node> node;
};

/// Multi-byte UTF-8 (2, 3 and 4 bytes per character): the length prefix counts bytes.
const char* const FRAME = "kamera_\xCE\xBA_\xE2\x82\xAC_\xF0\x9F\x93\xB7";

/// A head with every string and array that is not the loaned field filled, and the
/// fixed-size numbers after it set, so that a wrong offset shows.
template <typename M>
M headOf()
{
    M m{};
    if constexpr (requires { m.header.frame_id; }) {
        m.header.frame_id = FRAME;
        m.header.stamp.secs = 7;
        m.header.stamp.nsecs = 8;
    }
    if constexpr (requires { m.layout.dim; }) {
        m.layout.dim.resize(2);
        m.layout.dim[0].label = "rows_\xC3\xBC";
        m.layout.dim[0].size = 2;
        m.layout.dim[0].stride = 6;
        m.layout.dim[1].label = "c";
        m.layout.dim[1].size = 3;
        m.layout.dim[1].stride = 3;
        m.layout.data_offset = 1;
    }
    if constexpr (requires { m.encoding; }) {
        m.encoding = "bgr8";
        m.height = 2;
        m.width = 3;
        m.step = 9;
        m.is_bigendian = 1;
    }
    if constexpr (requires { m.format; }) {
        m.format = "jpeg";
    }
    if constexpr (requires { m.fields; }) {
        m.fields.resize(2);
        m.fields[0].name = "x";
        m.fields[0].offset = 0;
        m.fields[0].datatype = sensor_msgs::PointField::FLOAT32;
        m.fields[0].count = 1;
        m.fields[1].name = "intensit\xC3\xA4t";
        m.fields[1].offset = 4;
        m.fields[1].datatype = sensor_msgs::PointField::UINT8;
        m.fields[1].count = 1;
        m.height = 1;
        m.point_step = 5;
        m.is_bigendian = true;
        m.is_dense = true;  // after data: the tail
    }
    if constexpr (requires { m.info.resolution; }) {
        m.info.map_load_time.secs = 3;
        m.info.resolution = 0.05F;
        m.info.width = 4;
        m.info.height = 5;
        m.info.origin.position.x = 1.5;
        m.info.origin.orientation.w = 1.0;
    }
    return m;
}

template <typename M>
std::vector<uint8_t> wireOf(M message, uint32_t seq)
{
    if constexpr (requires { message.header.seq; }) {
        message.header.seq = seq;
    }
    return hypha::codec::encode(message);
}

/// The property over every loanable standard type, the 15 the generator marks. It holds the
/// payload bytes; that the headers agree but for the sequence and the send times (magic,
/// flags, schema hash, publisher id and payload_len equal) is held at the transport, by
/// hypha_iceoryx2's a_shrunk_loan_sends_the_frame_of_a_loan_of_its_kept_size. For each of
/// capacities 0, 1, 3 and 1001 elements, and each of used = all, 0 and half:
/// the loan's frame, and the frame of publish(msg) for the same message right after it,
/// both equal the reference encoding, with consecutive header.seq.
template <typename M>
void wireIdentity(TypedLoanContract& fixture, const std::string& topic)
{
    using Element = typename hypha::Loan<M>::Element;
    std::vector<std::vector<uint8_t>> seen;
    auto                              sub = fixture.template collect<M>(topic, seen);
    auto                              pub = hypha::Node().advertise<M>(hypha::Topic{topic});
    uint32_t                          seq = 0;
    size_t                            frames = 0;
    for (const size_t capacity : {size_t{0}, size_t{1}, size_t{3}, size_t{1001}}) {
        for (const size_t used : {capacity, size_t{0}, capacity / 2}) {
            auto loan = pub.loan(headOf<M>(), capacity * sizeof(Element));
            ASSERT_TRUE(loan.has_value()) << loan.error().name();
            ASSERT_EQ(loan->data().size(), capacity * sizeof(Element));
            for (size_t i = 0; i < loan->data().size(); ++i) {
                loan->data()[i] = static_cast<std::byte>((i * 31 + capacity + used) & 0xFF);
            }
            M expected = headOf<M>();
            expected.*hypha::message_traits::LoanedField<M>::member = std::vector<Element>(used);
            if (used != 0) {
                std::memcpy(
                    (expected.*hypha::message_traits::LoanedField<M>::member).data(),
                    loan->data().data(),
                    used * sizeof(Element)
                );
            }
            ASSERT_TRUE(pub.publish(std::move(*loan), used * sizeof(Element)))
                << pub.lastPublishStatus().name();
            ASSERT_TRUE(pub.publish(expected));
            fixture.deliver();
            ASSERT_EQ(seen.size(), frames + 2);
            EXPECT_EQ(seen[frames], wireOf(expected, seq))
                << "capacity " << capacity << " used " << used;
            EXPECT_EQ(seen[frames + 1], wireOf(expected, seq + 1));
            frames += 2;
            seq += 2;
        }
    }
}

}  // namespace

#define WIRE_IDENTITY(Type, name)                                                                  \
    TEST_F(TypedLoanContract, WireIdentity_##name)                                                 \
    {                                                                                              \
        wireIdentity<Type>(*this, testTopic("/t"));                                                \
    }

WIRE_IDENTITY(sensor_msgs::Image, Image)
WIRE_IDENTITY(sensor_msgs::CompressedImage, CompressedImage)
WIRE_IDENTITY(sensor_msgs::PointCloud2, PointCloud2)
WIRE_IDENTITY(nav_msgs::OccupancyGrid, OccupancyGrid)
WIRE_IDENTITY(std_msgs::ByteMultiArray, ByteMultiArray)
WIRE_IDENTITY(std_msgs::Int8MultiArray, Int8MultiArray)
WIRE_IDENTITY(std_msgs::UInt8MultiArray, UInt8MultiArray)
WIRE_IDENTITY(std_msgs::Int16MultiArray, Int16MultiArray)
WIRE_IDENTITY(std_msgs::UInt16MultiArray, UInt16MultiArray)
WIRE_IDENTITY(std_msgs::Int32MultiArray, Int32MultiArray)
WIRE_IDENTITY(std_msgs::UInt32MultiArray, UInt32MultiArray)
WIRE_IDENTITY(std_msgs::Int64MultiArray, Int64MultiArray)
WIRE_IDENTITY(std_msgs::UInt64MultiArray, UInt64MultiArray)
WIRE_IDENTITY(std_msgs::Float32MultiArray, Float32MultiArray)
WIRE_IDENTITY(std_msgs::Float64MultiArray, Float64MultiArray)

// The integrity code covers the shrunk payload: a byte changed after the publish, through a
// span the docs say not to keep, is rejected by the subscriber as a corrupt payload.
TEST_F(TypedLoanContract, AByteChangedThroughAStaleSpanAfterThePublishIsRejected)
{
    const std::string topic = testTopic("/cloud");
    size_t            received = 0;
    auto              sub = hypha::Node().subscribe<sensor_msgs::PointCloud2>(
        hypha::Topic{topic}, [&received](const hypha::MessagePtr<sensor_msgs::PointCloud2>&) {
            ++received;
        }
    );
    auto pub = hypha::Node().advertise<sensor_msgs::PointCloud2>(hypha::Topic{topic});

    auto good = pub.loan(headOf<sensor_msgs::PointCloud2>(), 10 * 5);
    ASSERT_TRUE(good.has_value());
    ASSERT_TRUE(pub.publish(std::move(*good), 7 * 5));  // shrunk: verifies
    this->deliver();
    EXPECT_EQ(received, 1U);

    auto loan = pub.loan(headOf<sensor_msgs::PointCloud2>(), 10 * 5);
    ASSERT_TRUE(loan.has_value());
    const std::span<std::byte> stale = loan->data();
    ASSERT_TRUE(pub.publish(std::move(*loan), 7 * 5));
    stale[0] ^= std::byte{0xFF};  // after the publish, before the subscriber reads
    this->deliver();
    EXPECT_EQ(received, 1U);
    EXPECT_EQ(sub.metrics().corruptPayload, 1U);
}

// A third loan while two are held is LoanLimit, and not a lost frame: every frame the driver
// published arrives and none is counted missing.
TEST_F(TypedLoanContract, AThirdLoanRefusedWhileTwoAreHeldIsLoanLimitAndNotALostFrame)
{
    const std::string topic = testTopic("/image");
    size_t            received = 0;
    auto              sub = hypha::Node().subscribe<sensor_msgs::Image>(
        hypha::Topic{topic}, [&received](const hypha::MessagePtr<sensor_msgs::Image>&) {
            ++received;
        }
    );
    auto               pub = hypha::Node().advertise<sensor_msgs::Image>(hypha::Topic{topic});
    sensor_msgs::Image before = headOf<sensor_msgs::Image>();
    before.data.assign(18, 1);
    ASSERT_TRUE(pub.publish(before));  // the subscriber knows the sequence from here
    this->deliver();
    auto first = pub.loan(headOf<sensor_msgs::Image>(), 18);
    auto second = pub.loan(headOf<sensor_msgs::Image>(), 18);
    ASSERT_TRUE(first.has_value() && second.has_value());
    const auto third = pub.loan(headOf<sensor_msgs::Image>(), 18);
    ASSERT_FALSE(third.has_value());
    EXPECT_EQ(third.error(), hypha::Status::LoanLimit);
    ASSERT_TRUE(pub.publish(std::move(*first)));
    ASSERT_TRUE(pub.publish(std::move(*second)));
    this->deliver();
    EXPECT_EQ(received, 3U);
    EXPECT_EQ(sub.missingSequences(), 0U) << "a refused loan is reported as a lost frame";
}

// A moved-from loan (publish takes the loan by move) is empty: no capacity, no bytes.
TEST_F(TypedLoanContract, AMovedFromLoanHasNoBytes)
{
    auto pub = hypha::Node().advertise<sensor_msgs::Image>(hypha::Topic{testTopic("/image")});
    auto loan = pub.loan(headOf<sensor_msgs::Image>(), 18);
    ASSERT_TRUE(loan.has_value());
    ASSERT_TRUE(pub.publish(std::move(*loan)));
    EXPECT_EQ(loan->capacity(), 0U);
    EXPECT_TRUE(loan->data().empty());
    auto other = pub.loan(headOf<sensor_msgs::Image>(), 18);
    ASSERT_TRUE(other.has_value());
    hypha::Loan<sensor_msgs::Image> moved = std::move(*other);
    EXPECT_EQ(other->capacity(), 0U);
    EXPECT_TRUE(other->data().empty());
    EXPECT_EQ(moved.capacity(), 18U);
}

// A moved-from loan published again is refused and takes no header.seq.
TEST_F(TypedLoanContract, PublishingAMovedFromLoanTakesNoSequence)
{
    const std::string                 topic = testTopic("/image");
    std::vector<std::vector<uint8_t>> seen;
    auto                              sub = this->collect<sensor_msgs::Image>(topic, seen);
    auto               pub = hypha::Node().advertise<sensor_msgs::Image>(hypha::Topic{topic});
    sensor_msgs::Image plain{};  // empty strings: a moved-from head has the same shape
    plain.width = 2;
    auto loan = pub.loan(plain, 4);
    ASSERT_TRUE(loan.has_value());
    std::memset(loan->data().data(), 0, 4);
    ASSERT_TRUE(pub.publish(std::move(*loan)));
    EXPECT_FALSE(pub.publish(std::move(*loan)));
    EXPECT_EQ(pub.lastPublishStatus(), hypha::Status::InvalidArgument);
    plain.data.assign(4, 0);
    ASSERT_TRUE(pub.publish(plain));
    this->deliver();
    ASSERT_EQ(seen.size(), 2U);
    EXPECT_EQ(seen[1], wireOf(plain, 1)) << "the refused publish took header.seq 1";
}

// A loan of one publisher, published through another of the same type: refused, nothing sent
// on either topic, and the loan's own publisher still sends with its next header.seq.
TEST_F(TypedLoanContract, ALoanOfAnotherPublisherIsRefused)
{
    const std::string                 topicA = testTopic("/a");
    const std::string                 topicB = testTopic("/b");
    std::vector<std::vector<uint8_t>> seenA;
    std::vector<std::vector<uint8_t>> seenB;
    auto                              subA = this->collect<sensor_msgs::Image>(topicA, seenA);
    auto                              subB = this->collect<sensor_msgs::Image>(topicB, seenB);
    auto pubA = hypha::Node().advertise<sensor_msgs::Image>(hypha::Topic{topicA});
    auto pubB = hypha::Node().advertise<sensor_msgs::Image>(hypha::Topic{topicB});
    auto loan = pubA.loan(headOf<sensor_msgs::Image>(), 18);
    ASSERT_TRUE(loan.has_value());
    std::memset(loan->data().data(), 1, 18);
    const bool sent = pubB.publish(std::move(*loan));
    this->deliver();
    EXPECT_FALSE(sent) << "sent to A: " << seenA.size() << ", to B: " << seenB.size();
    EXPECT_EQ(pubB.lastPublishStatus(), hypha::Status::InvalidArgument);
    EXPECT_TRUE(seenA.empty());
    EXPECT_TRUE(seenB.empty());
    sensor_msgs::Image whole = headOf<sensor_msgs::Image>();
    whole.data.assign(18, 1);
    ASSERT_TRUE(pubB.publish(whole));
    this->deliver();
    ASSERT_EQ(seenB.size(), 1U);
    EXPECT_EQ(seenB[0], wireOf(whole, 0)) << "the refused loan took B's header.seq";
}

// Sizes that are not whole elements.
TEST_F(TypedLoanContract, PartialElementsAreRefused)
{
    auto pub = hypha::Node().advertise<std_msgs::UInt16MultiArray>(hypha::Topic{testTopic("/u16")});
    const auto odd = pub.loan(headOf<std_msgs::UInt16MultiArray>(), 3);
    ASSERT_FALSE(odd.has_value());
    EXPECT_EQ(odd.error(), hypha::Status::InvalidArgument);
    auto loan = pub.loan(headOf<std_msgs::UInt16MultiArray>(), 4);
    ASSERT_TRUE(loan.has_value());
    EXPECT_FALSE(pub.publish(std::move(*loan), 3));
    EXPECT_EQ(pub.lastPublishStatus(), hypha::Status::InvalidArgument);
}

// The shape check holds the lengths of the head's strings and arrays, not their contents: a
// changed content, even two strings changed in opposite directions, is sent as that message;
// a resized array or a filled loaned field is refused.
TEST_F(TypedLoanContract, HeadChangesOfTheSameLengthsAreSentAndOthersRefused)
{
    using sensor_msgs::PointCloud2;
    const std::string                 topic = testTopic("/cloud");
    std::vector<std::vector<uint8_t>> seen;
    auto                              sub = this->collect<PointCloud2>(topic, seen);
    auto pub = hypha::Node().advertise<PointCloud2>(hypha::Topic{topic});

    // Same length, other content: sent, and the bytes are those of the changed message.
    auto same = pub.loan(headOf<PointCloud2>(), 5);
    ASSERT_TRUE(same.has_value());
    std::memset(same->data().data(), 9, 5);
    same->message().fields[0].name = "y";
    ASSERT_TRUE(pub.publish(std::move(*same)));
    // Two strings changed in opposite directions: the shape check cannot see it; the
    // encode is still right because the whole head is re-encoded.
    auto swapped = pub.loan(headOf<PointCloud2>(), 5);
    ASSERT_TRUE(swapped.has_value());
    std::memset(swapped->data().data(), 9, 5);
    swapped->message().fields[0].name = "xx";
    swapped->message().header.frame_id.pop_back();
    EXPECT_TRUE(pub.publish(std::move(*swapped)));
    // A resized array of the head: refused.
    auto resized = pub.loan(headOf<PointCloud2>(), 5);
    ASSERT_TRUE(resized.has_value());
    resized->message().fields.pop_back();
    EXPECT_FALSE(pub.publish(std::move(*resized)));
    // A filled loaned field at the publish: refused.
    auto filled = pub.loan(headOf<PointCloud2>(), 5);
    ASSERT_TRUE(filled.has_value());
    filled->message().data = {1};
    EXPECT_FALSE(pub.publish(std::move(*filled)));
    this->deliver();

    PointCloud2 expected = headOf<PointCloud2>();
    expected.data.assign(5, 9);
    expected.fields[0].name = "y";
    ASSERT_GE(seen.size(), 1U);
    EXPECT_EQ(seen[0], wireOf(expected, 0));
    expected.fields[0].name = "xx";
    expected.header.frame_id.pop_back();
    ASSERT_EQ(seen.size(), 2U);
    EXPECT_EQ(seen[1], wireOf(expected, 1));
}

// An unwritten loan sends zeros (iceoryx2 fills the loan), never the bytes of an earlier frame.
TEST_F(TypedLoanContract, AnUnwrittenLoanSendsZerosNotAnEarlierFrame)
{
    const std::string                 topic = testTopic("/image");
    std::vector<std::vector<uint8_t>> seen;
    auto                              sub = this->collect<sensor_msgs::Image>(topic, seen);
    auto             pub = hypha::Node().advertise<sensor_msgs::Image>(hypha::Topic{topic});
    constexpr size_t SIZE = 4096;
    for (int round = 0; round < 8; ++round) {
        sensor_msgs::Image full = headOf<sensor_msgs::Image>();
        full.data.assign(SIZE, 0xAB);
        ASSERT_TRUE(pub.publish(full));
        this->deliver();
        auto loan = pub.loan(headOf<sensor_msgs::Image>(), SIZE);
        ASSERT_TRUE(loan.has_value());
        ASSERT_TRUE(pub.publish(std::move(*loan)));
        this->deliver();
    }
    ASSERT_EQ(seen.size(), 16U);
    for (size_t frame = 1; frame < seen.size(); frame += 2) {
        const auto& bytes = seen[frame];
        size_t      stale = 0;
        for (size_t i = bytes.size() - SIZE; i < bytes.size(); ++i) {
            stale += bytes[i] == 0xAB ? 1 : 0;
        }
        EXPECT_EQ(stale, 0U) << "frame " << frame;
    }
}

// A loan returns its slot when an exception unwinds past it and when another thread drops it.
TEST_F(TypedLoanContract, LoansReturnOnExceptionsAndOtherThreads)
{
    auto pub = hypha::Node().advertise<sensor_msgs::Image>(hypha::Topic{testTopic("/image")});
    try {
        auto a = pub.loan(headOf<sensor_msgs::Image>(), 18);
        auto b = pub.loan(headOf<sensor_msgs::Image>(), 18);
        ASSERT_TRUE(a.has_value() && b.has_value());
        throw std::runtime_error("decoder failed");
    } catch (const std::runtime_error&) {
    }
    auto a = pub.loan(headOf<sensor_msgs::Image>(), 18);
    auto b = pub.loan(headOf<sensor_msgs::Image>(), 18);
    ASSERT_TRUE(a.has_value() && b.has_value());
    std::thread([moved = std::move(*a)]() mutable {
        (void)moved.data();
    }).join();
    std::thread([moved = std::move(*b)]() mutable {
        (void)moved.data();
    }).join();
    auto c = pub.loan(headOf<sensor_msgs::Image>(), 18);
    auto d = pub.loan(headOf<sensor_msgs::Image>(), 18);
    EXPECT_TRUE(c.has_value() && d.has_value());
}

// A loan that outlives its publisher keeps its slot mapped until it is dropped (the ASan
// build of these tests checks the write).
TEST_F(TypedLoanContract, ALoanOutlivesItsPublisher)
{
    std::optional<hypha::Loan<sensor_msgs::Image>> kept;
    {
        auto pub = hypha::Node().advertise<sensor_msgs::Image>(hypha::Topic{testTopic("/image")});
        auto loan = pub.loan(headOf<sensor_msgs::Image>(), 4096);
        ASSERT_TRUE(loan.has_value());
        kept.emplace(std::move(*loan));
    }
    std::memset(kept->data().data(), 0x5A, kept->data().size());  // ASan: still mapped?
    kept.reset();
}

// A loan whose node is gone is refused with InvalidState at the publish.
TEST_F(TypedLoanContract, ALoanOutlivesItsNode)
{
    auto pub = hypha::Node().advertise<sensor_msgs::Image>(hypha::Topic{testTopic("/image")});
    auto loan = pub.loan(headOf<sensor_msgs::Image>(), 4096);
    ASSERT_TRUE(loan.has_value());
    this->node.reset();
    std::memset(loan->data().data(), 0x5A, loan->data().size());
    EXPECT_FALSE(pub.publish(std::move(*loan)));
    EXPECT_EQ(pub.lastPublishStatus(), hypha::Status::InvalidState);
}

// data() has no alignment (the ROS 1 encoding has none: a Float64MultiArray's numbers start
// at 4 modulo 8 here); put() writes each number with memcpy, and they arrive intact.
TEST_F(TypedLoanContract, PutWritesNumbersAtAnUnalignedDataStart)
{
    const std::string                 topic = testTopic("/f");
    std::vector<std::vector<uint8_t>> seen;
    auto                              sub = this->collect<std_msgs::Float64MultiArray>(topic, seen);
    auto pub = hypha::Node().advertise<std_msgs::Float64MultiArray>(hypha::Topic{topic});
    auto loan = pub.loan(headOf<std_msgs::Float64MultiArray>(), 8 * sizeof(double));
    ASSERT_TRUE(loan.has_value());
    const auto address = reinterpret_cast<std::uintptr_t>(loan->data().data());
    EXPECT_NE(address % alignof(double), 0U) << "this head happens to align data(): test weaker";
    std_msgs::Float64MultiArray expected = headOf<std_msgs::Float64MultiArray>();
    for (size_t i = 0; i < 8; ++i) {
        const double value = 0.5 + static_cast<double>(i);
        loan->put(i, value);
        expected.data.push_back(value);
    }
    ASSERT_TRUE(pub.publish(std::move(*loan)));
    this->deliver();
    ASSERT_EQ(seen.size(), 1U);
    EXPECT_EQ(seen[0], wireOf(expected, 0));
}

// publish(msg) while two loans are held is LoanLimit, and there the message is dropped: the
// subscriber counts one lost frame.
TEST_F(TypedLoanContract, APublishRefusedWhileTwoLoansAreHeldIsLoanLimitAndALostFrame)
{
    const std::string topic = testTopic("/image");
    auto              sub = hypha::Node().subscribe<sensor_msgs::Image>(
        hypha::Topic{topic}, [](const hypha::MessagePtr<sensor_msgs::Image>&) {
        }
    );
    auto               pub = hypha::Node().advertise<sensor_msgs::Image>(hypha::Topic{topic});
    sensor_msgs::Image whole = headOf<sensor_msgs::Image>();
    whole.data.assign(18, 1);
    ASSERT_TRUE(pub.publish(whole));
    this->deliver();
    auto first = pub.loan(headOf<sensor_msgs::Image>(), 18);
    auto second = pub.loan(headOf<sensor_msgs::Image>(), 18);
    ASSERT_TRUE(first.has_value() && second.has_value());
    EXPECT_FALSE(pub.publish(whole));
    EXPECT_EQ(pub.lastPublishStatus(), hypha::Status::LoanLimit);
    ASSERT_TRUE(pub.publish(std::move(*first)));
    this->deliver();
    EXPECT_EQ(sub.missingSequences(), 1U);
}
