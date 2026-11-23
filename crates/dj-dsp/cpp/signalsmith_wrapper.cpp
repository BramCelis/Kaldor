// C interface to Signalsmith Stretch for dj-dsp (see src/signalsmith.rs).
//
// Written for Kaldor. Signalsmith Stretch itself (third_party/) is MIT
// licensed, (c) Geraint Luff / Signalsmith Audio Ltd.
//
// All memory is allocated in djs_create. djs_reset, djs_seek and
// djs_process reuse it: the library sizes its working buffers in
// configure(), and every later resize stays within that capacity.
// No C++ exception crosses this interface.

#include "signalsmith-stretch/signalsmith-stretch.h"

#include <cstddef>
#include <new>

namespace {

using Stretch = signalsmith::stretch::SignalsmithStretch<float>;

// Gives buffer[channel][frame] access to an interleaved buffer.
template <class T>
class Interleaved {
    T *data;
    int channels;

public:
    Interleaved(T *data, int channels) : data(data), channels(channels) {}

    class Channel {
        T *data;
        int channel;
        int stride;

    public:
        Channel(T *data, int channel, int stride) : data(data), channel(channel), stride(stride) {}
        T &operator[](std::size_t frame) const { return data[frame * stride + channel]; }
    };

    Channel operator[](std::size_t channel) const { return Channel(data, int(channel), channels); }
};

} // namespace

extern "C" {

// Returns nullptr if allocation fails.
void *djs_create(int channels, float sample_rate, int split_computation) {
    Stretch *s = new (std::nothrow) Stretch();
    if (s == nullptr) return nullptr;
    try {
        s->presetDefault(channels, sample_rate, split_computation != 0);
    } catch (...) {
        delete s;
        return nullptr;
    }
    return s;
}

void djs_destroy(void *handle) { delete static_cast<Stretch *>(handle); }

void djs_reset(void *handle) { static_cast<Stretch *>(handle)->reset(); }

// Pitch shift in semitones (key shift with key lock). Only sets members.
void djs_set_transpose(void *handle, double semitones) {
    static_cast<Stretch *>(handle)->setTransposeSemitones(float(semitones));
}

int djs_input_latency(const void *handle) {
    return static_cast<const Stretch *>(handle)->inputLatency();
}

int djs_output_latency(const void *handle) {
    return static_cast<const Stretch *>(handle)->outputLatency();
}

int djs_block_samples(const void *handle) {
    return static_cast<const Stretch *>(handle)->blockSamples();
}

int djs_interval_samples(const void *handle) {
    return static_cast<const Stretch *>(handle)->intervalSamples();
}

// `input` holds `frames` interleaved frames with the configured channel count.
void djs_seek(void *handle, const float *input, int frames, int channels, double rate) {
    Interleaved<const float> in(input, channels);
    static_cast<Stretch *>(handle)->seek(in, frames, rate);
}

void djs_process(void *handle, const float *input, int input_frames, float *output,
                 int output_frames, int channels) {
    Interleaved<const float> in(input, channels);
    Interleaved<float> out(output, channels);
    static_cast<Stretch *>(handle)->process(in, input_frames, out, output_frames);
}

} // extern "C"
