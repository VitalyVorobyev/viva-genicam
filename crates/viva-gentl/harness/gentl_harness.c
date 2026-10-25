/*
 * A GenTL consumer in C, compiled against the EMVA's own GenTL.h, that
 * drives viva_gentl.cti the way an application does: load it, list the
 * interfaces and devices, open the camera, read every module's GenApi XML
 * through its port, and acquire frames from viva-fake-gige.
 *
 * It checks what a C compiler can check and the Rust tests cannot: that the
 * producer's constants, structure layouts and calling convention agree with
 * the standard's header. A mismatch shows up as a wrong answer here.
 *
 * Usage:
 *   gentl_harness <path/to/viva_gentl.cti> <xml-out-dir> <frames>
 *                 <AcquisitionStart address> <AcquisitionStop address>
 *                 [<model>]
 *
 * The camera is the fake's Mono16 image, whose bytes are the ramp
 * (i * 7 + block id) mod 256; the acquisition registers are the fake's own
 * (viva_fake_gige::REG_ACQ_START / REG_ACQ_STOP). Exit status 0 on success.
 *
 * With <model>, the camera is the one the producer lists under that model,
 * and each frame is checked only for completeness and for the bytes its
 * width, height and pixel format need. An acquisition register is written 1
 * unless given as <address>=<value>, for a camera whose XML starts and stops
 * through one register (an Allied Vision Manta: 0x130F4=1 and 0x130F4=0).
 *
 * POSIX only (dlopen): CI runs it on Linux.
 */

#include <dlfcn.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "GenTL.h"

#define FAKE_MODEL "FakeGigE"
#define MONO16 0x01100007ULL
#define BUFFERS 4
#define BUFFER_BYTES (4u * 1024u * 1024u)

static int failures = 0;

#define CHECK(cond, ...)                                                       \
  do {                                                                         \
    if (!(cond)) {                                                             \
      fprintf(stderr, "FAIL %s:%d: ", __FILE__, __LINE__);                     \
      fprintf(stderr, __VA_ARGS__);                                            \
      fprintf(stderr, "\n");                                                   \
      failures++;                                                              \
    }                                                                          \
  } while (0)

/* Every function of GenTL 1.6, resolved by name as a consumer does. */
#define GENTL_FUNCTIONS(X)                                                     \
  X(GCGetInfo) X(GCGetLastError) X(GCInitLib) X(GCCloseLib) X(GCReadPort)      \
  X(GCWritePort) X(GCGetPortURL) X(GCGetPortInfo) X(GCRegisterEvent)           \
  X(GCUnregisterEvent) X(EventGetData) X(EventGetDataInfo) X(EventGetInfo)     \
  X(EventFlush) X(EventKill) X(TLOpen) X(TLClose) X(TLGetInfo)                 \
  X(TLGetNumInterfaces) X(TLGetInterfaceID) X(TLGetInterfaceInfo)              \
  X(TLOpenInterface) X(TLUpdateInterfaceList) X(IFClose) X(IFGetInfo)          \
  X(IFGetNumDevices) X(IFGetDeviceID) X(IFUpdateDeviceList)                    \
  X(IFGetDeviceInfo) X(IFOpenDevice) X(DevGetPort) X(DevGetNumDataStreams)     \
  X(DevGetDataStreamID) X(DevOpenDataStream) X(DevGetInfo) X(DevClose)         \
  X(DSAnnounceBuffer) X(DSAllocAndAnnounceBuffer) X(DSFlushQueue)              \
  X(DSStartAcquisition) X(DSStopAcquisition) X(DSGetInfo) X(DSGetBufferID)     \
  X(DSClose) X(DSRevokeBuffer) X(DSQueueBuffer) X(DSGetBufferInfo)             \
  X(GCGetNumPortURLs) X(GCGetPortURLInfo) X(GCReadPortStacked)                 \
  X(GCWritePortStacked) X(DSGetBufferChunkData) X(IFGetParentTL)               \
  X(DevGetParentIF) X(DSGetParentDev) X(DSGetNumBufferParts)                   \
  X(DSGetBufferPartInfo) X(DSAnnounceCompositeBuffer)                          \
  X(DSGetBufferInfoStacked) X(DSGetBufferPartInfoStacked) X(DSGetNumFlows)     \
  X(DSGetFlowInfo) X(DSGetNumBufferSegments) X(DSGetBufferSegmentInfo)

#define DECLARE(name) static P##name p##name;
GENTL_FUNCTIONS(DECLARE)

static int load(const char *path) {
  void *lib = dlopen(path, RTLD_NOW | RTLD_LOCAL);
  if (!lib) {
    fprintf(stderr, "cannot load %s: %s\n", path, dlerror());
    return 0;
  }
  int count = 0;
#define RESOLVE(name)                                                          \
  p##name = (P##name)dlsym(lib, #name);                                        \
  CHECK(p##name != NULL, "%s is not exported (§6.1: all are mandatory)",       \
        #name);                                                                \
  count++;
  GENTL_FUNCTIONS(RESOLVE)
  printf("resolved %d GenTL functions\n", count);
  return failures == 0;
}

static void last_error(const char *call, GC_ERROR rc) {
  GC_ERROR code = 0;
  char text[1024] = {0};
  size_t size = sizeof text;
  pGCGetLastError(&code, text, &size);
  fprintf(stderr, "  %s returned %d; last error %d: %s\n", call, (int)rc,
          (int)code, text);
}

#define GC(call)                                                               \
  do {                                                                         \
    GC_ERROR rc_ = (call);                                                     \
    if (rc_ != GC_ERR_SUCCESS) {                                               \
      last_error(#call, rc_);                                                  \
      CHECK(0, "%s", #call);                                                   \
    }                                                                          \
  } while (0)

/* A string info: size first, then the value. */
typedef GC_ERROR (*InfoFn)(void *ctx, int32_t cmd, INFO_DATATYPE *type,
                           void *buffer, size_t *size);

static int string_info(InfoFn fn, void *ctx, int32_t cmd, char *out,
                       size_t cap) {
  INFO_DATATYPE type = INFO_DATATYPE_UNKNOWN;
  size_t size = 0;
  if (fn(ctx, cmd, &type, NULL, &size) != GC_ERR_SUCCESS || size > cap)
    return 0;
  if (fn(ctx, cmd, &type, out, &size) != GC_ERR_SUCCESS)
    return 0;
  CHECK(type == INFO_DATATYPE_STRING, "command %d is not a string", (int)cmd);
  CHECK(size >= 1 && out[size - 1] == '\0', "string %d is not 0-terminated",
        (int)cmd);
  return 1;
}

static GC_ERROR tl_info(void *ctx, int32_t cmd, INFO_DATATYPE *t, void *b,
                        size_t *s) {
  return pTLGetInfo((TL_HANDLE)ctx, cmd, t, b, s);
}
static GC_ERROR dev_info(void *ctx, int32_t cmd, INFO_DATATYPE *t, void *b,
                         size_t *s) {
  return pDevGetInfo((DEV_HANDLE)ctx, cmd, t, b, s);
}
static GC_ERROR port_info(void *ctx, int32_t cmd, INFO_DATATYPE *t, void *b,
                          size_t *s) {
  return pGCGetPortInfo((PORT_HANDLE)ctx, cmd, t, b, s);
}

/* Read a port's first XML (GenTL §4.1.3's example) and write it to
 * <dir>/<name>.xml. */
static void save_port_xml(PORT_HANDLE port, const char *dir,
                          const char *name) {
  uint32_t urls = 0;
  GC(pGCGetNumPortURLs(port, &urls));
  CHECK(urls >= 1, "%s: no URL", name);
  if (urls == 0)
    return;
  char url[2048] = {0};
  size_t size = sizeof url;
  INFO_DATATYPE type = 0;
  GC(pGCGetPortURLInfo(port, 0, URL_INFO_URL, &type, url, &size));
  char file[512] = {0};
  unsigned long long address = 0, length = 0;
  /* local:[///]file;address;length[?SchemaVersion=...] */
  const char *body = url + 6; /* after "local:" or "Local:" */
  while (*body == '/')
    body++;
  CHECK(sscanf(body, "%511[^;];%llx;%llx", file, &address, &length) == 3,
        "%s: not a local URL: %s", name, url);
  char *xml = malloc(length + 1);
  size_t got = (size_t)length;
  GC(pGCReadPort(port, address, xml, &got));
  CHECK(got == length, "%s: read %zu of %llu bytes", name, got, length);
  xml[length] = '\0';
  CHECK(strstr(xml, "<RegisterDescription") != NULL, "%s: not GenApi XML",
        name);
  char path[1024];
  snprintf(path, sizeof path, "%s/%s.xml", dir, name);
  FILE *out = fopen(path, "wb");
  CHECK(out != NULL, "cannot write %s", path);
  if (out) {
    fwrite(xml, 1, strlen(xml), out);
    fclose(out);
  }
  printf("  %-10s %s (%llu bytes) -> %s\n", name, url, length, path);
  free(xml);
}

/* `value` as a GigE Vision register holds it: big-endian. */
static uint32_t be32(uint32_t value) {
  uint8_t bytes[4] = {(uint8_t)(value >> 24), (uint8_t)(value >> 16),
                      (uint8_t)(value >> 8), (uint8_t)value};
  uint32_t out;
  memcpy(&out, bytes, sizeof out);
  return out;
}

int main(int argc, char **argv) {
  if (argc != 6 && argc != 8) {
    fprintf(stderr,
            "usage: %s <cti> <xml-out-dir> <frames> <acq-start>[=value] "
            "<acq-stop>[=value] [model payload-size]\n",
            argv[0]);
    return 2;
  }
  const char *cti = argv[1], *dir = argv[2];
  int frames = atoi(argv[3]);
  char *rest = NULL;
  uint64_t acq_start = strtoull(argv[4], &rest, 0);
  uint32_t start_value = *rest == '=' ? (uint32_t)strtoul(rest + 1, NULL, 0) : 1;
  uint64_t acq_stop = strtoull(argv[5], &rest, 0);
  uint32_t stop_value = *rest == '=' ? (uint32_t)strtoul(rest + 1, NULL, 0) : 1;
  const char *model_wanted = argc == 8 ? argv[6] : FAKE_MODEL;
  int fake = strcmp(model_wanted, FAKE_MODEL) == 0;
  size_t expected_payload = argc == 8 ? (size_t)strtoull(argv[7], NULL, 0) : 0;
  if (!fake && (expected_payload == 0 || expected_payload > BUFFER_BYTES)) {
    fprintf(stderr, "real-camera payload-size must fit the harness buffers\n");
    return 2;
  }
  if (!load(cti))
    return 1;

  GC(pGCInitLib());
  CHECK(pGCInitLib() == GC_ERR_RESOURCE_IN_USE, "second GCInitLib");

  TL_HANDLE tl = GENTL_INVALID_HANDLE;
  GC(pTLOpen(&tl));
  char text[4096];
  CHECK(string_info(tl_info, tl, TL_INFO_TLTYPE, text, sizeof text) &&
            strcmp(text, TLTypeGEVName) == 0,
        "TL_INFO_TLTYPE");
  CHECK(string_info(tl_info, tl, TL_INFO_VENDOR, text, sizeof text),
        "TL_INFO_VENDOR");
  printf("producer: %s", text);
  CHECK(string_info(tl_info, tl, TL_INFO_PATHNAME, text, sizeof text),
        "TL_INFO_PATHNAME");
  printf(" at %s\n", text);
  {
    uint32_t major = 0, minor = 0;
    INFO_DATATYPE type = 0;
    size_t size = sizeof major;
    GC(pTLGetInfo(tl, TL_INFO_GENTL_VER_MAJOR, &type, &major, &size));
    size = sizeof minor;
    GC(pTLGetInfo(tl, TL_INFO_GENTL_VER_MINOR, &type, &minor, &size));
    CHECK(type == INFO_DATATYPE_UINT32 && major == GenTLMajorVersion &&
              minor == GenTLMinorVersion,
          "GenTL version %u.%u, header %d.%d", major, minor, GenTLMajorVersion,
          GenTLMinorVersion);
  }
  save_port_xml(tl, dir, "system");

  /* Interfaces, then the camera on one of them. */
  bool8_t changed = 0;
  GC(pTLUpdateInterfaceList(tl, &changed, 1000));
  uint32_t ifaces = 0;
  GC(pTLGetNumInterfaces(tl, &ifaces));
  printf("%u interfaces\n", ifaces);
  IF_HANDLE iface = GENTL_INVALID_HANDLE;
  DEV_HANDLE dev = GENTL_INVALID_HANDLE;
  for (uint32_t i = 0; i < ifaces && dev == GENTL_INVALID_HANDLE; i++) {
    char id[512];
    size_t size = sizeof id;
    GC(pTLGetInterfaceID(tl, i, id, &size));
    IF_HANDLE candidate = GENTL_INVALID_HANDLE;
    GC(pTLOpenInterface(tl, id, &candidate));
    GC(pIFUpdateDeviceList(candidate, &changed, 500));
    uint32_t devices = 0;
    GC(pIFGetNumDevices(candidate, &devices));
    printf("  interface %s: %u devices\n", id, devices);
    for (uint32_t d = 0; d < devices; d++) {
      char dev_id[512], model[512];
      size_t dsize = sizeof dev_id, msize = sizeof model;
      INFO_DATATYPE type = 0;
      GC(pIFGetDeviceID(candidate, d, dev_id, &dsize));
      GC(pIFGetDeviceInfo(candidate, dev_id, DEVICE_INFO_MODEL, &type, model,
                          &msize));
      printf("    %s: %s\n", dev_id, model);
      if (strcmp(model, model_wanted) == 0) {
        GC(pIFOpenDevice(candidate, dev_id, DEVICE_ACCESS_CONTROL, &dev));
        iface = candidate;
        break;
      }
    }
    if (iface == GENTL_INVALID_HANDLE)
      GC(pIFClose(candidate));
  }
  CHECK(dev != GENTL_INVALID_HANDLE, "no camera %s was found", model_wanted);
  if (dev == GENTL_INVALID_HANDLE)
    goto close;
  save_port_xml(iface, dir, "interface");
  save_port_xml(dev, dir, "device");
  {
    TL_HANDLE parent = GENTL_INVALID_HANDLE;
    GC(pIFGetParentTL(iface, &parent));
    CHECK(parent == tl, "IFGetParentTL");
  }

  PORT_HANDLE remote = GENTL_INVALID_HANDLE;
  GC(pDevGetPort(dev, &remote));
  save_port_xml(remote, dir, "remote");
  CHECK(string_info(port_info, remote, PORT_INFO_MODULE, text, sizeof text) &&
            strcmp(text, TLRemoteDeviceModuleName) == 0,
        "PORT_INFO_MODULE of the remote port: %s", text);
  CHECK(string_info(dev_info, dev, DEVICE_INFO_TLTYPE, text, sizeof text) &&
            strcmp(text, TLTypeGEVName) == 0,
        "DEVICE_INFO_TLTYPE");

  /* PORT_REGISTER_STACK_ENTRY: two reads of the first URL register. */
  {
    char a[4] = {0}, b[4] = {0};
    PORT_REGISTER_STACK_ENTRY entries[2] = {{0x0200, a, 4}, {0x0204, b, 4}};
    size_t n = 2;
    GC(pGCReadPortStacked(remote, entries, &n));
    CHECK(n == 2 && memcmp(a, "Loca", 4) == 0, "stacked read: %.4s", a);
  }

  /* The data stream. */
  uint32_t streams = 0;
  GC(pDevGetNumDataStreams(dev, &streams));
  CHECK(streams == 1, "%u data streams", streams);
  char stream_id[256];
  size_t ssize = sizeof stream_id;
  GC(pDevGetDataStreamID(dev, 0, stream_id, &ssize));
  DS_HANDLE ds = GENTL_INVALID_HANDLE;
  GC(pDevOpenDataStream(dev, stream_id, &ds));
  save_port_xml(ds, dir, "datastream");
  {
    bool8_t defines = 1;
    INFO_DATATYPE type = 0;
    size_t size = sizeof defines;
    GC(pDSGetInfo(ds, STREAM_INFO_DEFINES_PAYLOADSIZE, &type, &defines,
                  &size));
    CHECK(type == INFO_DATATYPE_BOOL8 && defines == 0,
          "STREAM_INFO_DEFINES_PAYLOADSIZE");
  }

  /* Without GenApi the harness cannot read PayloadSize; a generous buffer
   * is allowed (§5.2.1: "If the buffers are larger than requested it does
   * not matter"). */
  void *memory[BUFFERS];
  BUFFER_HANDLE buffers[BUFFERS];
  for (int i = 0; i < BUFFERS; i++) {
    memory[i] = calloc(1, BUFFER_BYTES);
    GC(pDSAnnounceBuffer(ds, memory[i], BUFFER_BYTES, (void *)(intptr_t)(i + 1),
                         &buffers[i]));
    GC(pDSQueueBuffer(ds, buffers[i]));
  }
  EVENT_HANDLE event = GENTL_INVALID_HANDLE;
  GC(pGCRegisterEvent(ds, EVENT_NEW_BUFFER, &event));
  GC(pDSStartAcquisition(ds, ACQ_START_FLAGS_DEFAULT, GENTL_INFINITE));
  {
    uint32_t value = be32(start_value); /* a GigE Vision register */
    size_t size = sizeof value;
    GC(pGCWritePort(remote, acq_start, &value, &size));
  }

  uint64_t last = 0;
  for (int n = 0; n < frames; n++) {
    EVENT_NEW_BUFFER_DATA data;
    size_t size = sizeof data;
    GC_ERROR rc = pEventGetData(event, &data, &size, 3000);
    if (rc != GC_ERR_SUCCESS) {
      last_error("EventGetData", rc);
      CHECK(0, "frame %d did not arrive", n);
      break;
    }
    CHECK(size == sizeof data, "EVENT_NEW_BUFFER_DATA is %zu bytes", size);
    {
      void *id = NULL;
      INFO_DATATYPE type = 0;
      size_t isize = sizeof id;
      GC(pEventGetDataInfo(event, &data, sizeof data, EVENT_DATA_ID, &type, &id,
                           &isize));
      CHECK(id == data.BufferHandle, "EVENT_DATA_ID");
    }
    int index = (int)(intptr_t)data.pUserPointer - 1;
    CHECK(index >= 0 && index < BUFFERS && buffers[index] == data.BufferHandle,
          "the private pointer names the buffer");
    if (index < 0 || index >= BUFFERS)
      break;

    /* Several infos at once: DS_BUFFER_INFO_STACKED's layout. */
    uint64_t frame_id = 0, pixel_format = 0;
    size_t width = 0, height = 0, filled = 0;
    bool8_t incomplete = 1;
    DS_BUFFER_INFO_STACKED infos[] = {
        {BUFFER_INFO_FRAMEID, 0, &frame_id, sizeof frame_id, -1},
        {BUFFER_INFO_PIXELFORMAT, 0, &pixel_format, sizeof pixel_format, -1},
        {BUFFER_INFO_WIDTH, 0, &width, sizeof width, -1},
        {BUFFER_INFO_HEIGHT, 0, &height, sizeof height, -1},
        {BUFFER_INFO_SIZE_FILLED, 0, &filled, sizeof filled, -1},
        {BUFFER_INFO_IS_INCOMPLETE, 0, &incomplete, sizeof incomplete, -1},
    };
    size_t count = sizeof infos / sizeof infos[0];
    GC(pDSGetBufferInfoStacked(ds, data.BufferHandle, infos, count));
    for (size_t i = 0; i < count; i++)
      CHECK(infos[i].iResult == GC_ERR_SUCCESS, "stacked info %d: %d",
            (int)infos[i].iInfoCmd, (int)infos[i].iResult);
    CHECK(infos[0].iType == INFO_DATATYPE_UINT64, "FRAMEID type");
    CHECK(infos[2].iType == INFO_DATATYPE_SIZET, "WIDTH type");
    CHECK(!incomplete, "frame %llu is incomplete",
          (unsigned long long)frame_id);
    CHECK(frame_id > last, "frame %llu after %llu",
          (unsigned long long)frame_id, (unsigned long long)last);
    last = frame_id;
    if (!fake) {
      /* PayloadSize read from the camera before starting: PFNC effective
       * bits alone do not account for legacy packed groups or padding. */
      CHECK(width > 0 && height > 0 && pixel_format != 0,
            "frame geometry and format must be present");
      CHECK(filled == expected_payload, "%zu bytes, camera advertises %zu", filled,
            expected_payload);
      printf("frame %llu: %zux%zu, pixel format 0x%08llx, %zu bytes, "
             "complete\n",
             (unsigned long long)frame_id, width, height,
             (unsigned long long)pixel_format, filled);
      GC(pDSQueueBuffer(ds, data.BufferHandle));
      continue;
    }
    CHECK(pixel_format == MONO16, "pixel format %llx",
          (unsigned long long)pixel_format);
    CHECK(filled == width * height * 2, "%zu bytes for %zux%zu Mono16", filled,
          width, height);
    const uint8_t *pixels = memory[index];
    size_t wrong = 0;
    for (size_t i = 0; i < filled; i++) {
      uint8_t expected = (uint8_t)(i * 7u + (uint16_t)frame_id);
      if (pixels[i] != expected)
        wrong++;
    }
    CHECK(wrong == 0, "frame %llu: %zu of %zu bytes differ from the ramp",
          (unsigned long long)frame_id, wrong, filled);
    printf("frame %llu: %zux%zu Mono16, %zu bytes, pixels match\n",
           (unsigned long long)frame_id, width, height, filled);
    GC(pDSQueueBuffer(ds, data.BufferHandle));
  }

  {
    uint32_t value = be32(stop_value);
    size_t size = sizeof value;
    GC(pGCWritePort(remote, acq_stop, &value, &size));
  }
  GC(pDSStopAcquisition(ds, ACQ_STOP_FLAGS_DEFAULT));
  GC(pDSFlushQueue(ds, ACQ_QUEUE_ALL_DISCARD));
  for (int i = 0; i < BUFFERS; i++) {
    void *base = NULL, *priv = NULL;
    GC(pDSRevokeBuffer(ds, buffers[i], &base, &priv));
    CHECK(base == memory[i] && priv == (void *)(intptr_t)(i + 1),
          "DSRevokeBuffer hands back the announced memory");
    free(memory[i]);
  }
  GC(pGCUnregisterEvent(ds, EVENT_NEW_BUFFER));
  GC(pDSClose(ds));
  GC(pDevClose(dev));
  GC(pIFClose(iface));

close:
  GC(pTLClose(tl));
  GC(pGCCloseLib());
  CHECK(pGCCloseLib() == GC_ERR_NOT_INITIALIZED, "second GCCloseLib");
  if (failures) {
    fprintf(stderr, "%d check(s) failed\n", failures);
    return 1;
  }
  printf("OK: %d frames through %s\n", frames, cti);
  return 0;
}
