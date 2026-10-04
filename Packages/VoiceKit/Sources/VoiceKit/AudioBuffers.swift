import AVFoundation
import Darwin

/// Host-time seconds, the clock of `AVAudioTime.hostTime` (tap timestamps) — so latencies measured
/// against tap buffers compare like with like.
enum HostClock {
    static func now() -> TimeInterval { AVAudioTime.seconds(forHostTime: mach_absolute_time()) }

    static func seconds(of time: AVAudioTime) -> TimeInterval {
        time.isHostTimeValid ? AVAudioTime.seconds(forHostTime: time.hostTime) : now()
    }
}

enum AudioLevel {
    /// Linear RMS (0…1) of channel 0 over `frames`.
    static func rms(_ buffer: AVAudioPCMBuffer, frames: Range<Int>) -> Float {
        guard !frames.isEmpty else { return 0 }
        var sum: Float = 0
        if let data = buffer.floatChannelData?[0] {
            for i in frames { sum += data[i] * data[i] }
        } else if let data = buffer.int16ChannelData?[0] {
            for i in frames { let s = Float(data[i]) / 32768; sum += s * s }
        } else {
            return 0
        }
        return (sum / Float(frames.count)).squareRoot()
    }

    /// RMS → 0…1 on a -50…-6 dBFS scale, which is what a waveform or a mouth should follow.
    static func display(_ rms: Float) -> Double {
        guard rms > 0 else { return 0 }
        return min(1, max(0, (20 * log10(Double(rms)) + 50) / 44))
    }

    /// Fast attack, slow release.
    static func smoothed(_ current: Double, toward target: Double) -> Double {
        current + (target - current) * (target > current ? 0.6 : 0.15)
    }
}

extension AVAudioPCMBuffer {
    /// Channel 0 as a fresh mono float buffer. Voice processing sometimes reports multi-channel input
    /// (one channel per mic); recognisers want mono, and the copy decouples us from the tap's buffer reuse.
    func monoCopy() -> AVAudioPCMBuffer? {
        guard let format = AVAudioFormat(standardFormatWithSampleRate: format.sampleRate, channels: 1),
              let copy = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: max(frameLength, 1)) else { return nil }
        let count = Int(frameLength)
        copy.frameLength = frameLength
        let out = copy.floatChannelData![0]
        if let data = floatChannelData?[0] {
            if self.format.isInterleaved {
                let stride = Int(self.format.channelCount)
                for i in 0..<count { out[i] = data[i * stride] }
            } else {
                out.update(from: data, count: count)
            }
        } else if let data = int16ChannelData?[0] {
            let stride = self.format.isInterleaved ? Int(self.format.channelCount) : 1
            for i in 0..<count { out[i] = Float(data[i * stride]) / 32768 }
        } else {
            return nil
        }
        return copy
    }

    /// Int16 LE mono samples of channel 0 (used to turn system TTS output into a `PCMChunk`).
    func int16Data() -> Data {
        let count = Int(frameLength)
        var samples = [Int16](repeating: 0, count: count)
        if let data = floatChannelData?[0] {
            for i in 0..<count { samples[i] = Int16(max(-1, min(1, data[i])) * 32767) }
        } else if let data = int16ChannelData?[0] {
            for i in 0..<count { samples[i] = data[i] }
        } else if let data = int32ChannelData?[0] {
            for i in 0..<count { samples[i] = Int16(truncatingIfNeeded: data[i] >> 16) }
        }
        return samples.withUnsafeBufferPointer { Data(buffer: $0) }
    }
}

extension PCMChunk {
    /// The chunk as an Int16 mono buffer in its own sample rate.
    func pcmBuffer() -> AVAudioPCMBuffer? {
        let frames = frameCount
        guard frames > 0, sampleRate > 0,
              let format = AVAudioFormat(commonFormat: .pcmFormatInt16, sampleRate: sampleRate, channels: 1, interleaved: false),
              let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(frames)) else { return nil }
        buffer.frameLength = AVAudioFrameCount(frames)
        // Byte copy: `Data` gives no alignment guarantee for Int16.
        samples.copyBytes(to: UnsafeMutableRawBufferPointer(start: buffer.int16ChannelData![0], count: frames * 2), count: frames * 2)
        return buffer
    }
}

/// `AVAudioConverter` wrapper that rebuilds itself when the input format changes.
///
/// `streaming: true` keeps resampler state between calls (continuous mic audio); `false` treats each
/// buffer as a complete stream and flushes the resampler tail (one TTS sentence per call).
final class FormatConverter {
    let outputFormat: AVAudioFormat
    private var converter: AVAudioConverter?

    init(outputFormat: AVAudioFormat) {
        self.outputFormat = outputFormat
    }

    func convert(_ input: AVAudioPCMBuffer, streaming: Bool) -> AVAudioPCMBuffer? {
        if input.format == outputFormat { return input }
        if converter?.inputFormat != input.format {
            converter = AVAudioConverter(from: input.format, to: outputFormat)
        }
        guard let converter else { return nil }
        if !streaming { converter.reset() }
        let ratio = outputFormat.sampleRate / input.format.sampleRate
        let capacity = AVAudioFrameCount((Double(input.frameLength) * ratio).rounded(.up)) + 1024
        guard let output = AVAudioPCMBuffer(pcmFormat: outputFormat, frameCapacity: capacity) else { return nil }

        let feed = InputFeed(buffer: input, endOfStream: !streaming)
        var error: NSError?
        let status = converter.convert(to: output, error: &error) { _, status in feed.next(status) }
        guard status != .error, output.frameLength > 0 else { return nil }
        return output
    }

    /// Hands the converter one buffer, then "no more for now" (streaming) or "end of stream".
    private final class InputFeed {
        let buffer: AVAudioPCMBuffer
        let endOfStream: Bool
        var delivered = false

        init(buffer: AVAudioPCMBuffer, endOfStream: Bool) {
            self.buffer = buffer
            self.endOfStream = endOfStream
        }

        func next(_ status: UnsafeMutablePointer<AVAudioConverterInputStatus>) -> AVAudioBuffer? {
            if delivered {
                status.pointee = endOfStream ? .endOfStream : .noDataNow
                return nil
            }
            delivered = true
            status.pointee = .haveData
            return buffer
        }
    }
}
