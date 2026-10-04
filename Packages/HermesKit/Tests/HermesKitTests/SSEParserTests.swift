import Foundation
import Testing
@testable import HermesKit

@Suite("SSE parser")
struct SSEParserTests {
    /// Feeds `text` split into chunks of `size` bytes.
    private func parse(_ text: String, chunkSize size: Int = 1_000_000) -> [SSEEvent] {
        var parser = SSEParser()
        let bytes = Array(text.utf8)
        var events: [SSEEvent] = []
        for start in stride(from: 0, to: bytes.count, by: size) {
            events += parser.push(bytes[start..<min(start + size, bytes.count)])
        }
        if let last = parser.finish() { events.append(last) }
        return events
    }

    @Test func dispatchesOnBlankLine() {
        let events = parse("event: tool.started\ndata: {\"a\":1}\nid: 7\n\ndata: second\n\n")
        #expect(events == [
            SSEEvent(event: "tool.started", data: "{\"a\":1}", id: "7"),
            SSEEvent(data: "second"),
        ])
    }

    @Test func joinsMultiLineData() {
        #expect(parse("data: line one\ndata: line two\ndata:\n\n") == [SSEEvent(data: "line one\nline two\n")])
    }

    @Test func ignoresCommentsAndKeepalives() {
        let events = parse(": keepalive\n\n:another\ndata: x\n: inline comment\n\n: keepalive\n\n")
        #expect(events == [SSEEvent(data: "x")])
    }

    @Test(arguments: ["\r\n", "\r", "\n"])
    func handlesLineEndings(_ newline: String) {
        let text = ["event: e", "data: a", "data: b", "", "data: c", "", ""].joined(separator: newline)
        for size in [1, 2, 3, 100] {
            #expect(parse(text, chunkSize: size) == [SSEEvent(event: "e", data: "a\nb"), SSEEvent(data: "c")])
        }
    }

    @Test func handlesUTF8SplitAcrossChunks() {
        let text = "data: Ça va ? 😀 …\n\n"
        for size in 1...6 {
            #expect(parse(text, chunkSize: size) == [SSEEvent(data: "Ça va ? 😀 …")])
        }
    }

    @Test func parsesFieldVariants() {
        let events = parse("data:no-space\nretry: 3000\nevent\nunknown: field\n\n")
        #expect(events == [SSEEvent(event: nil, data: "no-space", retry: 3000)])
    }

    @Test func skipsFramesWithoutData() {
        #expect(parse("event: ping\n\nid: 3\n\n").isEmpty)
    }

    @Test func stripsBOMAndFlushesTrailingFrameAtEOF() {
        #expect(parse("\u{FEFF}data: first\n\ndata: tail") == [SSEEvent(data: "first"), SSEEvent(data: "tail")])
    }

    @Test func worksOverAsyncByteSequences() async throws {
        let text = "data: one\r\n\r\n: keepalive\r\n\r\ndata: two\r\n\r\n"
        let stream = AsyncStream<UInt8> { continuation in
            for byte in text.utf8 { continuation.yield(byte) }
            continuation.finish()
        }
        var received: [String] = []
        for try await event in stream.sseEvents { received.append(event.data) }
        #expect(received == ["one", "two"])
    }
}
