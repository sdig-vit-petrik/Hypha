#include <hypha/hypha_ffi.h>

int main(void)
{
    if (hypha_abi_revision() != HYPHA_ABI_REVISION) {
        return 2;
    }
    struct HyphaNode* node = 0;
    const HyphaStatus status = hypha_node_create(0, 0, 0, 0, 0, &node);

    if (status != HYPHA_STATUS_INVALID_ARGUMENT) {
        return 1;
    }
    if (node != 0) {
        return 1;
    }
    return 0;
}
