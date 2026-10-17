import Foundation
import Testing

// Small adapters preserve the upstream regression cases while using Swift
// Testing, which is available in the standalone Command Line Tools toolchain.
private func failure(_ text: String, _ file: String, _ line: Int) {
    Issue.record(Comment(rawValue: text), sourceLocation: SourceLocation(fileID: file, filePath: file, line: line, column: 1))
}
func checkEqual<T: Equatable>(_ a: @autoclosure () throws -> T, _ b: @autoclosure () throws -> T, file: String = #filePath, line: Int = #line) {
    do { if try a() != b() { failure("Expected equal values", file, line) } } catch { failure("Unexpected error: \(error)", file, line) }
}
func checkEqual(_ a: @autoclosure () throws -> Double, _ b: @autoclosure () throws -> Double, accuracy: Double, file: String = #filePath, line: Int = #line) {
    do { if try abs(a() - b()) > accuracy { failure("Values exceeded tolerance", file, line) } } catch { failure("Unexpected error: \(error)", file, line) }
}
func checkNotEqual<T: Equatable>(_ a: @autoclosure () throws -> T, _ b: @autoclosure () throws -> T, file: String = #filePath, line: Int = #line) {
    do { if try a() == b() { failure("Expected distinct values", file, line) } } catch { failure("Unexpected error: \(error)", file, line) }
}
func checkTrue(_ a: @autoclosure () throws -> Bool, file: String = #filePath, line: Int = #line) {
    do { if try !a() { failure("Expected true", file, line) } } catch { failure("Unexpected error: \(error)", file, line) }
}
func checkFalse(_ a: @autoclosure () throws -> Bool, file: String = #filePath, line: Int = #line) {
    do { if try a() { failure("Expected false", file, line) } } catch { failure("Unexpected error: \(error)", file, line) }
}
func checkNil<T>(_ a: @autoclosure () throws -> T?, file: String = #filePath, line: Int = #line) {
    do { if try a() != nil { failure("Expected nil", file, line) } } catch { failure("Unexpected error: \(error)", file, line) }
}
func checkThrows<T>(_ a: @autoclosure () throws -> T, file: String = #filePath, line: Int = #line) {
    do { _ = try a(); failure("Expected an error", file, line) } catch {}
}
func checkGreater<T: Comparable>(_ a: @autoclosure () throws -> T, _ b: @autoclosure () throws -> T, file: String = #filePath, line: Int = #line) {
    do { if try a() <= b() { failure("Expected a larger value", file, line) } } catch { failure("Unexpected error: \(error)", file, line) }
}
struct MissingTestValue: Error {}
func requireValue<T>(_ value: T?) throws -> T {
    guard let value else { throw MissingTestValue() }
    return value
}
