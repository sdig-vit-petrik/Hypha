// Must NOT compile: the module name holds a character outside A-Z a-z 0-9 _ /.
#include <hypha_log/hypha_log.h>

HYPHA_LOG_MODULE_INIT("bad name")

void useIt()
{
    HYPHA_LOG_ERROR("never built");
}
