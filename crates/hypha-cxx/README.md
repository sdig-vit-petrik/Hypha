# hypha

The C++ client SDK: nodes, publishers, subscribers, parameters and timers, over
the C ABI exported by the hypha shared library.

One include gives you the whole API:

```cpp
#include <hypha/hypha.h>
```

Documentation:

- [Getting started](../../docs/getting-started.md)
- [C++ API](../../docs/cpp-api.md) — the task guide; the public headers' comments are the reference
- [Doxygen reference](../../docs/Doxyfile) — generate with `doxygen docs/Doxyfile`
- [Architecture](../../docs/architecture.md) — where this sits in the stack

The [speed example](../../examples/speed)
is a complete node package built against the SDK.
