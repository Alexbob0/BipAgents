import Foundation

/// One dispatched Server-Sent Events frame.
public struct SSEEvent: Sendable, Hashable {
    /// The `event:` field; `nil` means the default type (`message`).
    public var event: String?
    /// All `data:` lines joined with `\n`.
    public var data: String
    /// The `id:` field of this frame (not the sticky last-event-id).
    public var id: String?
    /// The `retry:` field in milliseconds, if present in this frame.
    public var retry: Int?

    public init(event: String? = nil, data: String, id: String? = nil, retry: Int? = nil) {
        self.event = event
        self.data = data
        self.id = id
        self.retry = retry
    }
}

/// Incremental, push-based SSE parser (WHATWG EventSource rules).
///
/// Feed raw bytes in arbitrary chunks. Lines end with LF, CRLF or CR; frames dispatch on a blank
/// line; comment lines (`:` — e.g. Hermes' `: keepalive`) are ignored. Lines are decoded only once
/// complete, so UTF-8 sequences split across chunks are handled naturally.
public struct SSEParser: Sendable {
    private var line: [UInt8] = []
    private var lastWasCR = false
    private var atStreamStart = true

    private var eventType: String?
    private var dataLines: [String] = []
    private var hasData = false
    private var id: String?
    private var retry: Int?

    public init() {}

    /// Consumes one byte; returns an event when a frame completes.
    public mutating func push(_ byte: UInt8) -> SSEEvent? {
        switch byte {
        case 0x0A: // LF
            if lastWasCR { lastWasCR = false; return nil } // second half of CRLF
            return endLine()
        case 0x0D: // CR
            lastWasCR = true
            return endLine()
        default:
            lastWasCR = false
            line.append(byte)
            return nil
        }
    }

    /// Consumes a chunk of bytes; returns every event it completes.
    public mutating func push<Bytes: Sequence<UInt8>>(_ bytes: Bytes) -> [SSEEvent] {
        bytes.compactMap { push($0) }
    }

    /// Call at end of stream. Leniently dispatches a trailing frame that was not followed by a blank line.
    public mutating func finish() -> SSEEvent? {
        if !line.isEmpty { _ = endLine() }
        lastWasCR = false
        return dispatch()
    }

    private mutating func endLine() -> SSEEvent? {
        var bytes = line
        line.removeAll(keepingCapacity: true)
        if atStreamStart {
            atStreamStart = false
            if bytes.starts(with: [0xEF, 0xBB, 0xBF]) { bytes.removeFirst(3) } // UTF-8 BOM
        }
        if bytes.isEmpty { return dispatch() }
        process(String(decoding: bytes, as: UTF8.self))
        return nil
    }

    private mutating func process(_ line: String) {
        if line.hasPrefix(":") { return } // comment / keepalive
        let field: Substring
        var value: Substring
        if let colon = line.firstIndex(of: ":") {
            field = line[..<colon]
            value = line[line.index(after: colon)...]
            if value.hasPrefix(" ") { value = value.dropFirst() }
        } else {
            field = Substring(line)
            value = ""
        }
        switch field {
        case "event": eventType = String(value)
        case "data":
            dataLines.append(String(value))
            hasData = true
        case "id" where !value.contains("\0"): id = String(value)
        case "retry": retry = Int(value)
        default: break
        }
    }

    private mutating func dispatch() -> SSEEvent? {
        defer {
            eventType = nil
            dataLines.removeAll()
            hasData = false
            id = nil
            retry = nil
        }
        guard hasData else { return nil }
        let type = eventType.flatMap { $0.isEmpty ? nil : $0 }
        return SSEEvent(event: type, data: dataLines.joined(separator: "\n"), id: id, retry: retry)
    }
}

/// Adapts any byte `AsyncSequence` (e.g. `URLSession.AsyncBytes`) into a sequence of `SSEEvent`s.
public struct SSEEventSequence<Base: AsyncSequence>: AsyncSequence where Base.Element == UInt8 {
    public typealias Element = SSEEvent

    let base: Base

    public init(_ base: Base) { self.base = base }

    public struct AsyncIterator: AsyncIteratorProtocol {
        var base: Base.AsyncIterator
        var parser = SSEParser()
        var finished = false

        public mutating func next() async throws -> SSEEvent? {
            guard !finished else { return nil }
            while let byte = try await base.next() {
                if let event = parser.push(byte) { return event }
            }
            finished = true
            return parser.finish()
        }
    }

    public func makeAsyncIterator() -> AsyncIterator {
        AsyncIterator(base: base.makeAsyncIterator())
    }
}

extension AsyncSequence where Element == UInt8 {
    /// Parses this byte stream as Server-Sent Events.
    public var sseEvents: SSEEventSequence<Self> { SSEEventSequence(self) }
}
