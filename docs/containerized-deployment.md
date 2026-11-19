# Containerized deployment

How to run a hypha system with its nodes in Docker containers, split over
several Compose projects that are started, stopped and updated independently.

## 1. What the containers share

hypha's transport is iceoryx2 shared memory on one host. Two directories make
up the bus, and every container of a system mounts both:

| Path in the container | Holds | Shared as |
|---|---|---|
| `/dev/shm` | the message payloads and iceoryx2's bookkeeping segments | Docker tmpfs volume `hypha-shm` |
| `/tmp/iceoryx2` | the service and node registry, and the Unix sockets that wake subscribers | Docker tmpfs volume `hypha-iox` |

Nothing else is shared. The containers need no common network, no `ports:`,
and no shared PID or IPC namespace. Both volumes are Docker-managed tmpfs
volumes: they live in kernel memory, have their own size limit, and are not the
host's `/dev/shm`.

Every container of one system also has:

- **The same numeric `user:`** (UID and GID). iceoryx2 creates its files
  `0600` and its directories `0750`, so a process of another UID cannot use
  them.
- **The same `HYPHA_DOMAIN`.** A domain is one system. Processes in different
  domains do not see each other, even on the same volumes.
- **`HYPHA_BOUNDED_MEMORY=1` for a fixed shared-memory budget.** In a bounded
  deployment a publisher without a payload limit is refused, and so is one whose
  segment does not fit into the free space of `/dev/shm` (`ResourceLimit`). `0` or
  no variable leaves it off; any other value is refused.
- **The same service policies.** A topic's creator declares its policy (`HyphaQos`, from the
  C ABI or the SDK that wraps it); an opener that declares a different policy is refused with
  `QosMismatch`. A process that publishes a topic without declaring one gives it hypha's
  `default` policy when no process has opened it. A subscriber that declares nothing never
  creates a topic (a framework topic excepted): it waits until a publisher opens it.
- **The same hypha build.** The first endpoint of a topic fixes the service
  layout, and a different build can refuse to open it.

## 2. Example: three Compose projects

```text
deploy/
├── bus/compose.yaml          owns the two volumes; brought up first
├── producers/compose.yaml    sensor nodes
└── consumers/compose.yaml    planning and control nodes
```

### `bus/compose.yaml`

The bus project creates the volumes and keeps them mounted. Its only service
holds the mounts, so the bus keeps its contents while every node project is
restarted.

```yaml
name: robot-bus

services:
  holder:
    image: busybox
    command: ["sleep", "infinity"]
    init: true
    restart: unless-stopped
    volumes:
      - shm:/dev/shm
      - iox:/tmp/iceoryx2

volumes:
  shm:
    name: hypha-shm
    driver: local
    driver_opts: { type: tmpfs, device: tmpfs, o: "size=2g,mode=1777" }
  iox:
    name: hypha-iox
    driver: local
    driver_opts: { type: tmpfs, device: tmpfs, o: "size=64m,mode=1777" }
```

### `producers/compose.yaml`

```yaml
name: robot-producers

x-hypha: &hypha
  image: registry.example/robot-nodes:1.4.2   # the same hypha build everywhere
  init: true
  user: "1000:1000"                           # the same UID/GID everywhere
  restart: unless-stopped
  stop_grace_period: 5s
  environment:
    HYPHA_DOMAIN: robot                       # the same domain everywhere
  volumes:
    - hypha-shm:/dev/shm
    - hypha-iox:/tmp/iceoryx2

services:
  camera:
    <<: *hypha
    command: ["camera_node", "/etc/robot/camera.toml"]
  lidar:
    <<: *hypha
    command: ["lidar_node", "/etc/robot/lidar.toml"]

volumes:
  hypha-shm: { external: true }
  hypha-iox: { external: true }
```

### `consumers/compose.yaml`

Identical in its shared part; only the services differ.

```yaml
name: robot-consumers

x-hypha: &hypha
  image: registry.example/robot-nodes:1.4.2
  init: true
  user: "1000:1000"
  restart: unless-stopped
  stop_grace_period: 5s
  environment:
    HYPHA_DOMAIN: robot
  volumes:
    - hypha-shm:/dev/shm
    - hypha-iox:/tmp/iceoryx2

services:
  planner:
    <<: *hypha
    command: ["planner_node", "/etc/robot/planner.toml"]

volumes:
  hypha-shm: { external: true }
  hypha-iox: { external: true }
```

### Running it

```sh
docker compose -f bus/compose.yaml up -d          # once; creates hypha-shm and hypha-iox
docker compose -f producers/compose.yaml up -d
docker compose -f consumers/compose.yaml up -d

docker compose -f consumers/compose.yaml down     # the producers keep running
docker compose -f consumers/compose.yaml up -d    # the consumers rejoin
```

The node projects can start in any order. A subscriber that starts before its
publisher waits for it, and a latched topic hands its last sample to a
subscriber that joins later.

Without the bus project, create the volumes once by hand and keep the
`external: true` declarations:

```sh
docker volume create --driver local --opt type=tmpfs --opt device=tmpfs \
  --opt o=size=2g,mode=1777 hypha-shm
docker volume create --driver local --opt type=tmpfs --opt device=tmpfs \
  --opt o=size=64m,mode=1777 hypha-iox
```

## 3. Sizing the volumes

`hypha-shm` holds one data segment per publisher. A segment has slots for every
subscriber's queue, each of 2 borrowed samples, plus history and loans, each slot the
size of the topic's payload limit and 104 B of headers. The `default` policy (8
publishers, 16 subscribers, depth 32) gives 16 × (32 + 2) + 1 + 2 = 547 slots, so a topic
bounded to 64 KiB needs about 36 MB. The framework topics (`/clock`, `/clock_progress`, 32 subscribers,
depth 32) give 1091 slots. The table is for the 1091 slots:

| Payload limit of the topic | Segment |
|---|---|
| 8 B (`/clock`, exact for a fixed-size message) | 0.05 MB |
| 256 B | 0.4 MB |
| 64 KiB | 71 MB |
| 1 MiB | 1.1 GB |

A segment is a sparse file: only the slots that are written use memory. In
steady state a system uses a few megabytes, but the worst case is the sum of all
segments.

- **Too small a volume does not fail with an error. It kills the node with
  SIGBUS** (exit code 135) at the write that does not fit; on Linux the process writes one line
  that names the full `/dev/shm` first. On Linux, a bounded
  publisher logs at creation when its segment cannot fit into the free space:
  `topic '<t>' may need <n> bytes of shared memory for its publisher pool …`.
  Size the volume from those numbers, or lower the topic's payload limit
  (`hypha::maxPayloadDynamic(bytes)`) or the limits of its policy (`hypha::Qos`).
- Growable publishers (a variable-size message without `maxPayloadDynamic`) create
  segments later and are not checked.
- The `size=` of a tmpfs volume is a cap, not a reservation.
- **Memory limits.** A tmpfs page counts against the memory limit (`mem_limit`)
  of the container whose process wrote it. That is the publisher, for its
  samples. Include the written part of its segments in that limit.
- `hypha-iox` holds small files and sockets; 64 MB is ample.

## 4. Lifecycle

### Crashes and restarts

A node that crashes or is killed leaves its files on the volumes. The next node
start, and every service open, removes the resources of nodes whose process is
gone, so a restarted container rejoins without cleanup. In the study, a
restarted publisher reached its subscribers again after 13–15 ms (median), at
most 53 ms. `restart: unless-stopped` is enough.

`docker stop` sends SIGTERM. The C++ SDK stops the node's run loop and releases
its endpoints. `init: true` forwards the signal and reaps child processes.

### Updating a project

You can stop, pull and restart one node project while the others run, as long
as the new image carries the same hypha build. To change the hypha version,
stop every project of the domain and reset the volumes (§5) first.

### Host reboot, or a restart of Docker

A tmpfs volume keeps its **definition** across reboots, but not its
**contents**. The contents exist while at least one container mounts the
volume, and vanish when the last one unmounts it.

After a reboot:

1. The Docker daemon starts the containers whose restart policy is `always` or
   `unless-stopped`. `unless-stopped` skips a container that was stopped by
   hand before the reboot.
2. The first container that mounts `hypha-shm` or `hypha-iox` gets a new,
   empty tmpfs. The volumes are still defined, so the `external: true`
   projects find them.
3. The nodes start on an empty bus, in whatever order Docker starts them. This
   is the same as a first start, and no stale state from before the reboot can
   exist.

Nothing is needed at boot beyond the restart policies. The bus project's
`holder` is not needed for a reboot either. It only keeps the bus contents when
every node project is down at the same time.

Docker Desktop (macOS, Windows) runs these volumes inside its Linux VM. Quitting
Docker Desktop is a reboot in this sense.

### Resetting the bus

Stop every project of the domain, including the bus project, then remove and
recreate the volumes:

```sh
docker compose -f consumers/compose.yaml down
docker compose -f producers/compose.yaml down
docker compose -f bus/compose.yaml down
docker volume rm hypha-shm hypha-iox
docker compose -f bus/compose.yaml up -d
```

This is the container equivalent of `./clean-shm.sh`. Reset both volumes
together; one reset alone is refused (§6).

## 5. Security

Any process that can write to `hypha-shm` can change a sample:

- **before its publisher seals it.** The frame CRC then certifies the changed
  bytes.
- **after a subscriber validated it**, while the application reads it in place.
  Delivery is zero-copy.

The frame CRC detects neither change (`docs/architecture.md` §3.4). iceoryx2
maps a publisher's segment read-only in every subscriber, so another hypha node
cannot make such a change. But iceoryx2 does not lock the files: any process
with the domain's UID, or root, can open a segment for writing. One UID is one
trust domain, and the CRC catches faults, not a deliberate writer, who simply
recomputes it. Access control is the defence:

- Mount `hypha-shm` and `hypha-iox` only into hypha containers.
- Run them under one dedicated UID that no other container or host process
  uses.
- Do not mount the host's `/dev/shm` into the containers.
- An application that needs end-to-end protection against a faulty writer
  checksums its own payload.

## 6. When it does not work

Every node logs to its container's stderr (`docker compose logs <service>`).

| Message or symptom | Cause | Fix |
|---|---|---|
| `domain '<d>' belongs to uid <a>, this process runs as uid <b>` (node creation fails with `InvalidState`) | a container runs with another `user:` | give every container of the domain the same numeric `user:` |
| `/dev/shm and /tmp/iceoryx2 are not shared by the same processes (markers … vs …)` | a container lacks one of the two mounts, or only one volume was reset | mount both volumes everywhere; reset both together (§4) |
| `no publisher connected to topic '<t>' in domain '<d>' after <n> ms` | the publisher is not running, runs in another domain, or is on another `hypha-shm` | check `HYPHA_DOMAIN` and that every container mounts the same `hypha-shm` |
| exit code 135, `ending on SIGBUS, most likely because shared memory (/dev/shm) is full` at the crash | SIGBUS: `hypha-shm` is full | look for `may need <n> bytes of shared memory` at start; enlarge `size=` |
| `depth <d> exceeds the <n> the service's QoS policy allows` | a subscriber asks for a queue deeper than the topic's policy allows | use depth 0 (the service's depth) or at most `<n>`; a deeper queue needs a creator that declares a deeper policy |
| `topic '<t>' refused a frame: CRC mismatch` | something writes into `hypha-shm` | see §5 |
| `refuses loans although this publisher is below its loan limit` | the publisher's pool is exhausted or corrupted | restart that node |
| `the service looks half-created or its metadata under … is damaged` | a node died while it created a service, or its metadata is damaged | restart the topic's publisher; otherwise reset the bus (§4) |
| `a stale shared-memory segment or a different hypha build created this service` | containers run different hypha builds | use one hypha build in all containers; reset the bus |

## 7. Setups that do not work across projects

- **`tmpfs:`, `--tmpfs` or `shm_size:` on each service.** Every container gets
  its own private tmpfs, so nothing is shared.
- **`ipc: shareable` with `ipc: container:<x>`.** This shares `/dev/shm`
  through one container's IPC namespace. Every other project then depends on
  that container, and `/tmp/iceoryx2` still needs a shared volume.
- **The host's `/dev/shm` and `/tmp/iceoryx2` as bind mounts.** This works, but
  it exposes the bus to every host process with the UID and mixes it with
  other iceoryx2 users. Prefer the tmpfs volumes.
