int pipewire_start(unsigned int node_id, void* user_data,
                   void (*on_process)(void* user_data));

#if defined(PIPEWIRE_IMPLEMENTATION)

#include <errno.h>
#include <linux/dma-buf.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>

#include <libdrm/drm_fourcc.h>
#include <pipewire/pipewire.h>
#include <spa/debug/format.h>
#include <spa/debug/pod.h>
#include <spa/debug/types.h>
#include <spa/param/param.h>
#include <spa/param/video/format-utils.h>
#include <spa/param/video/type-info.h>

typedef struct Data {
    struct pw_main_loop* loop;
    struct pw_stream* stream;
    struct spa_video_info_raw format;

    void* extern_data;
    void (*on_process)(void*);
} Data;

static struct spa_pod* build_format(struct spa_pod_builder* builder,
                                    uint32_t format, uint64_t* modifiers,
                                    size_t n_modifiers) {
    struct spa_pod_frame format_frame;
    struct spa_rectangle resolution = SPA_RECTANGLE(1920, 1080);
    struct spa_rectangle min_resolution = SPA_RECTANGLE(1, 1);
    struct spa_rectangle max_resolution = SPA_RECTANGLE(8192, 4320);
    struct spa_fraction framerate = SPA_FRACTION(30, 1);
    struct spa_fraction min_framerate = SPA_FRACTION(0, 1);
    struct spa_fraction max_framerate = SPA_FRACTION(360, 1);

    /* Make an object of type SPA_TYPE_OBJECT_Format and id
     * SPA_PARAM_EnumFormat. The object type is important because it defines the
     * properties that are acceptable. The id gives more context about what the
     * object is meant to contain. In this case we enumerate supported formats.
     */
    spa_pod_builder_push_object(builder, &format_frame, SPA_TYPE_OBJECT_Format,
                                SPA_PARAM_EnumFormat);
    /* add media type and media subtype properties */
    spa_pod_builder_add(builder, SPA_FORMAT_mediaType,
                        SPA_POD_Id(SPA_MEDIA_TYPE_video), 0);
    spa_pod_builder_add(builder, SPA_FORMAT_mediaSubtype,
                        SPA_POD_Id(SPA_MEDIA_SUBTYPE_raw), 0);

    /* formats */
    spa_pod_builder_add(builder, SPA_FORMAT_VIDEO_format, SPA_POD_Id(format),
                        0);
    if (n_modifiers > 0) {
        struct spa_pod_frame modifier_frame;

        /* build an enumeration of modifiers */
        spa_pod_builder_prop(builder, SPA_FORMAT_VIDEO_modifier,
                             SPA_POD_PROP_FLAG_MANDATORY |
                                 SPA_POD_PROP_FLAG_DONT_FIXATE);

        spa_pod_builder_push_choice(builder, &modifier_frame, SPA_CHOICE_Enum,
                                    0);

        /* The first element of choice pods is the preferred value. Here
         * we arbitrarily pick the first modifier as the preferred one.
         */
        spa_pod_builder_long(builder, modifiers[0]);

        /* modifiers from an array */
        for (size_t i = 0; i < n_modifiers; i++)
            spa_pod_builder_long(builder, modifiers[i]);

        spa_pod_builder_pop(builder, &modifier_frame);
    }

    /* add size and framerate ranges */
    spa_pod_builder_add(builder, SPA_FORMAT_VIDEO_size,
                        SPA_POD_CHOICE_RANGE_Rectangle(
                            &resolution, &min_resolution, &max_resolution),
                        SPA_FORMAT_VIDEO_framerate,
                        SPA_POD_CHOICE_RANGE_Fraction(
                            &framerate, &min_framerate, &max_framerate),
                        0);

    return spa_pod_builder_pop(builder, &format_frame);
};

static void on_param_changed(void* user_data, uint32_t id,
                             const struct spa_pod* param) {
    Data* data = user_data;

    if (param == NULL || id != SPA_PARAM_Format)
        return;

    if (spa_format_video_raw_parse(param, &data->format) < 0) {
        printf("error parsing video format raw\n");
        return;
    };

    printf("got format:\n");
    spa_debug_format(0, NULL, param);

    uint8_t params_buffer[1024];
    struct spa_pod_builder builder =
        SPA_POD_BUILDER_INIT(params_buffer, sizeof(params_buffer));
    const int n_params = 1;
    const struct spa_pod* params[n_params];

    params[0] = spa_pod_builder_add_object(
        &builder, SPA_TYPE_OBJECT_ParamBuffers, SPA_PARAM_Buffers,
        SPA_PARAM_BUFFERS_buffers, SPA_POD_CHOICE_RANGE_Int(8, 1, 64),
        SPA_PARAM_BUFFERS_blocks, SPA_POD_Int(1), SPA_PARAM_BUFFERS_size,
        SPA_POD_Int(1920 * 1080), SPA_PARAM_BUFFERS_stride, SPA_POD_Int(1920),
        SPA_PARAM_BUFFERS_dataType,
        SPA_POD_CHOICE_FLAGS_Int((1 << SPA_DATA_DmaBuf)));

    if (pw_stream_update_params(data->stream, params, n_params) < 0) {
        printf("error updating stream params");
        return;
    }
}

static void on_process(void* user_data) {
    Data* data = user_data;

    struct pw_buffer* buffer;
    if ((buffer = pw_stream_dequeue_buffer(data->stream)) == NULL) {
        printf("dequeue buffer NULL\n");
        return;
    }

    printf("n_datas: %d\n", buffer->buffer->n_datas);
    printf("n_metas: %d\n", buffer->buffer->n_metas);
    printf("size: %d\n", buffer->buffer->datas[0].chunk->size);
    printf("stride: %d\n", buffer->buffer->datas[0].chunk->stride);
    printf("offset: %d\n", buffer->buffer->datas[0].chunk->offset);
    printf("type: %d\n", buffer->buffer->datas[0].type);
    printf("fd: %ld\n", buffer->buffer->datas[0].fd);
    printf("meta type: %u\n", buffer->buffer->metas[0].type);
    printf("\n");

    int64_t fd = buffer->buffer->datas[0].fd;
    int32_t stride = buffer->buffer->datas[0].chunk->stride;

    void* mapped;
    if ((mapped = mmap(NULL, stride * 1080, PROT_READ, MAP_SHARED, fd, 0)) ==
        MAP_FAILED) {
        printf("mmap error: %s\n", strerror(errno));
        return;
    };

    ioctl(fd, DMA_BUF_SYNC_START);

    // do something
    uint8_t* mapped_u8 = mapped;

    for (int i = 0; i < 100; i++) {
        uint8_t b = *mapped_u8++;
        uint8_t g = *mapped_u8++;
        uint8_t r = *mapped_u8++;
        uint8_t x = *mapped_u8++;
        printf("[%d] b: %u, g: %u, r: %u x: %u\n", i, b, g, r, x);
    }

    ioctl(fd, DMA_BUF_SYNC_END);

    if (munmap(mapped, stride * 1080) < 0) {
        printf("munmap error: %s\n", strerror(errno));
        return;
    }

    pw_stream_queue_buffer(data->stream, buffer);
}

static void state_changed(void* data, enum pw_stream_state old,
                          enum pw_stream_state state, const char* error) {
    printf("state: from %s to %s\n", pw_stream_state_as_string(old),
           pw_stream_state_as_string(state));
    if (error) {
        printf("error: %s\n", error);
    }
}

static const struct pw_stream_events stream_events = {
    PW_VERSION_STREAM_EVENTS,
    .param_changed = on_param_changed,
    .process = on_process,
    .state_changed = state_changed,
};

int pipewire_start(uint32_t node_id, void* user_data,
                   void (*on_process)(void* user_data)) {

    pw_init(0, NULL);

    Data data = {0};
    data.extern_data = user_data;
    data.on_process = on_process;

    uint8_t buffer[2048];
    struct spa_pod_builder builder =
        SPA_POD_BUILDER_INIT(buffer, sizeof(buffer));

    data.loop = pw_main_loop_new(NULL);

    struct pw_properties* props =
        pw_properties_new(PW_KEY_MEDIA_TYPE, "Video", PW_KEY_MEDIA_CATEGORY,
                          "Capture", PW_KEY_MEDIA_ROLE, "Screen", NULL);

    data.stream = pw_stream_new_simple(pw_main_loop_get_loop(data.loop),
                                       "screenshare-video-capture", props,
                                       &stream_events, &data);

    const int n_modifiers = 4;
    uint64_t modifiers[n_modifiers];
    modifiers[0] = DRM_FORMAT_MOD_LINEAR;
    modifiers[1] = DRM_FORMAT_MOD_LINEAR;
    modifiers[2] = 216172782120099857;
    modifiers[3] = DRM_FORMAT_MOD_INVALID;

    const int n_params = 5;
    const struct spa_pod* params[n_params];
    params[0] =
        build_format(&builder, SPA_VIDEO_FORMAT_RGB, modifiers, n_modifiers);
    params[1] =
        build_format(&builder, SPA_VIDEO_FORMAT_BGRx, modifiers, n_modifiers);
    params[2] =
        build_format(&builder, SPA_VIDEO_FORMAT_BGRA, modifiers, n_modifiers);
    params[3] =
        build_format(&builder, SPA_VIDEO_FORMAT_BGR, modifiers, n_modifiers);
    params[4] =
        build_format(&builder, SPA_VIDEO_FORMAT_RGBx, modifiers, n_modifiers);

    printf("connecting to node id: %d\n", node_id);

    if (pw_stream_connect(data.stream, PW_DIRECTION_INPUT, node_id,
                          PW_STREAM_FLAG_AUTOCONNECT |
                              PW_STREAM_FLAG_MAP_BUFFERS,
                          params, n_params) < 0) {
        return 1;
    }

    int ret = 0;
    ret = pw_main_loop_run(data.loop);

    pw_stream_destroy(data.stream);
    pw_main_loop_destroy(data.loop);
    pw_deinit();

    return ret;
}

#endif // PIPEWIRE_IMPLEMENTATION
