import Foundation
import Darwin

/// App-owned login files, protected by the current user's filesystem permissions.
/// Credentials are not encrypted and never enter settings, logs or diagnostics.
public struct SecretVault: Sendable {
    public let directory: URL
    private static let maximumSize = 1_048_576

    public init(directory: URL = FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent("Library/Application Support/AIUsage Lite/Credentials")) {
        self.directory = directory
    }

    public func load(_ id: UUID) throws -> Data? {
        guard let folder = try openDirectory(create: false) else { return nil }
        defer { close(folder) }
        let file = openat(folder, filename(id), O_RDONLY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC)
        if file < 0, errno == ENOENT { return nil }
        guard file >= 0 else { throw failure() }
        defer { close(file) }
        var info = stat()
        guard fstat(file, &info) == 0, info.st_mode & S_IFMT == S_IFREG,
              info.st_uid == geteuid(), info.st_nlink == 1,
              info.st_size >= 0, info.st_size <= Self.maximumSize,
              fchmod(file, 0o600) == 0 else { throw failure() }
        var data = Data(), buffer = [UInt8](repeating: 0, count: 8192)
        while true {
            let count = Darwin.read(file, &buffer, buffer.count)
            if count < 0, errno == EINTR { continue }
            guard count >= 0 else { throw failure() }
            if count == 0 { return data }
            guard data.count + count <= Self.maximumSize else { throw failure() }
            data.append(contentsOf: buffer.prefix(count))
        }
    }

    public func save(_ data: Data, id: UUID) throws {
        guard data.count <= Self.maximumSize, let folder = try openDirectory(create: true) else { throw failure() }
        defer { close(folder) }
        let temporary = ".\(UUID().uuidString).tmp"
        let file = openat(folder, temporary, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0o600)
        guard file >= 0 else { throw failure() }
        defer { close(file); unlinkat(folder, temporary, 0) }
        guard fchmod(file, 0o600) == 0 else { throw failure() }
        try data.withUnsafeBytes { bytes in
            var offset = 0
            while offset < bytes.count {
                let count = Darwin.write(file, bytes.baseAddress!.advanced(by: offset), bytes.count - offset)
                if count < 0, errno == EINTR { continue }
                guard count > 0 else { throw failure() }
                offset += count
            }
        }
        // Publish only a complete, private file. Replacing a symlink never follows it.
        guard fsync(file) == 0, renameat(folder, temporary, folder, filename(id)) == 0 else { throw failure() }
    }

    public func delete(_ id: UUID) throws {
        guard let folder = try openDirectory(create: false) else { return }
        defer { close(folder) }
        let status = unlinkat(folder, filename(id), 0)
        guard status == 0 || errno == ENOENT else { throw failure() }
    }

    private func filename(_ id: UUID) -> String { "\(id.uuidString).json" }
    private func openDirectory(create: Bool) throws -> Int32? {
        if create {
            do {
                try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true,
                    attributes: [.posixPermissions: 0o700])
            } catch { throw failure() }
        }
        let folder = open(directory.path, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
        if folder < 0, !create, errno == ENOENT { return nil }
        guard folder >= 0 else { throw failure() }
        var info = stat()
        guard fstat(folder, &info) == 0, info.st_uid == geteuid(), fchmod(folder, 0o700) == 0 else {
            close(folder); throw failure()
        }
        return folder
    }
    private func failure() -> ProviderError {
        ProviderError("credential_storage", "앱 전용 로그인 파일에 접근할 수 없습니다. 앱 데이터 폴더의 권한과 저장 공간을 확인하세요.")
    }
}
