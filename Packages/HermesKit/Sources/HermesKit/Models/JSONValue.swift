import Foundation

/// A lossless, `Sendable` representation of arbitrary JSON.
///
/// Used for lenient decoding of server payloads whose exact shape is not yet pinned down,
/// and to keep "raw" extra fields around for debugging and forward compatibility.
public enum JSONValue: Sendable, Hashable {
    case null
    case bool(Bool)
    case number(Double)
    case string(String)
    case array([JSONValue])
    case object([String: JSONValue])
}

// MARK: - Accessors

extension JSONValue {
    public subscript(key: String) -> JSONValue? {
        if case .object(let object) = self { object[key] } else { nil }
    }

    public var objectValue: [String: JSONValue]? {
        if case .object(let object) = self { object } else { nil }
    }

    public var arrayValue: [JSONValue]? {
        if case .array(let array) = self { array } else { nil }
    }

    /// The string payload, only for `.string`.
    public var stringValue: String? {
        if case .string(let string) = self { string } else { nil }
    }

    /// A string for `.string`, or a canonical rendering of numbers and booleans (`5476`, `1.5`, `true`).
    /// Handy for identifiers that servers emit either as strings or as integers.
    public var lenientString: String? {
        switch self {
        case .string(let string): string
        case .number(let number):
            number.rounded() == number && abs(number) < 1e15 ? String(Int64(number)) : String(number)
        case .bool(let bool): String(bool)
        default: nil
        }
    }

    public var doubleValue: Double? {
        switch self {
        case .number(let number): number
        case .string(let string): Double(string)
        default: nil
        }
    }

    public var intValue: Int? { doubleValue.map { Int($0) } }

    public var boolValue: Bool? {
        switch self {
        case .bool(let bool): bool
        case .number(let number): number != 0
        case .string(let string):
            switch string.lowercased() {
            case "true", "yes", "1": true
            case "false", "no", "0": false
            default: nil
            }
        default: nil
        }
    }

    public var isNull: Bool { self == .null }

    /// Accepts Unix seconds, Unix milliseconds, numeric strings and ISO 8601 strings
    /// (with or without fractional seconds or time zone; naive values are read as UTC).
    public var dateValue: Date? {
        switch self {
        case .number(let number):
            return Date(timeIntervalSince1970: number > 1e12 ? number / 1000 : number)
        case .string(let string):
            if let number = Double(string) { return JSONValue.number(number).dateValue }
            return Self.parseISODate(string)
        default:
            return nil
        }
    }

    private static func parseISODate(_ string: String) -> Date? {
        let withFraction = Date.ISO8601FormatStyle(includingFractionalSeconds: true)
        if let date = try? withFraction.parse(string) { return date }
        if let date = try? Date.ISO8601FormatStyle().parse(string) { return date }
        // Python `datetime.isoformat()` without a time zone, possibly with a space separator.
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = TimeZone(secondsFromGMT: 0)
        for format in ["yyyy-MM-dd'T'HH:mm:ss.SSSSSS", "yyyy-MM-dd'T'HH:mm:ss", "yyyy-MM-dd HH:mm:ss.SSSSSS", "yyyy-MM-dd HH:mm:ss"] {
            formatter.dateFormat = format
            if let date = formatter.date(from: string) { return date }
        }
        return nil
    }
}

// MARK: - Parsing / serialisation

extension JSONValue {
    public static func parse(_ data: Data) throws -> JSONValue {
        try JSONDecoder().decode(JSONValue.self, from: data)
    }

    public static func parse(_ string: String) throws -> JSONValue {
        try parse(Data(string.utf8))
    }

    /// Compact JSON with sorted keys (deterministic, convenient for tests and logs).
    public func encoded() throws -> Data {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        return try encoder.encode(self)
    }

    public var jsonString: String {
        (try? encoded()).flatMap { String(data: $0, encoding: .utf8) } ?? "null"
    }
}

extension JSONValue: Codable {
    public init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let bool = try? container.decode(Bool.self) {
            self = .bool(bool)
        } else if let number = try? container.decode(Double.self) {
            self = .number(number)
        } else if let string = try? container.decode(String.self) {
            self = .string(string)
        } else if let array = try? container.decode([JSONValue].self) {
            self = .array(array)
        } else {
            self = .object(try container.decode([String: JSONValue].self))
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .null: try container.encodeNil()
        case .bool(let bool): try container.encode(bool)
        case .number(let number):
            if number.rounded() == number, abs(number) < 1e15 {
                try container.encode(Int64(number))
            } else {
                try container.encode(number)
            }
        case .string(let string): try container.encode(string)
        case .array(let array): try container.encode(array)
        case .object(let object): try container.encode(object)
        }
    }
}

// MARK: - Literals

extension JSONValue: ExpressibleByNilLiteral, ExpressibleByBooleanLiteral, ExpressibleByIntegerLiteral,
    ExpressibleByFloatLiteral, ExpressibleByStringLiteral, ExpressibleByArrayLiteral, ExpressibleByDictionaryLiteral {
    public init(nilLiteral: ()) { self = .null }
    public init(booleanLiteral value: Bool) { self = .bool(value) }
    public init(integerLiteral value: Int) { self = .number(Double(value)) }
    public init(floatLiteral value: Double) { self = .number(value) }
    public init(stringLiteral value: String) { self = .string(value) }
    public init(arrayLiteral elements: JSONValue...) { self = .array(elements) }
    public init(dictionaryLiteral elements: (String, JSONValue)...) {
        self = .object(Dictionary(elements, uniquingKeysWith: { _, last in last }))
    }
}

// MARK: - Lenient field lookup

/// Looks up fields across a payload and its common nesting wrappers (`data`, `payload`),
/// trying several candidate keys in order. Keeps server-shape guesses out of call sites.
struct LenientFields: Sendable {
    private let layers: [[String: JSONValue]]

    init(_ value: JSONValue, nestedIn wrappers: [String] = ["data", "payload"]) {
        let root = value.objectValue ?? [:]
        layers = [root] + wrappers.compactMap { root[$0]?.objectValue }
    }

    /// First non-null value found for any of `keys` (keys take priority over layers).
    func value(_ keys: String...) -> JSONValue? { value(keys) }

    func value(_ keys: [String]) -> JSONValue? {
        for key in keys {
            for layer in layers {
                if let value = layer[key], !value.isNull { return value }
            }
        }
        return nil
    }

    func string(_ keys: String...) -> String? { value(keys)?.lenientString }
    func bool(_ keys: String...) -> Bool? { value(keys)?.boolValue }
    func double(_ keys: String...) -> Double? { value(keys)?.doubleValue }
    func int(_ keys: String...) -> Int? { value(keys)?.intValue }
    func date(_ keys: String...) -> Date? { value(keys)?.dateValue }

    /// Merged view of all layers (root wins), for collecting "extra" fields.
    var merged: [String: JSONValue] {
        layers.reversed().reduce(into: [:]) { result, layer in result.merge(layer) { _, new in new } }
    }
}
