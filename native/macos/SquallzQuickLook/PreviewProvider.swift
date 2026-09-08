import CoreGraphics
import Foundation
import QuickLookUI
import UniformTypeIdentifiers

@_silgen_name("squallz_quicklook_render")
private func renderSquallzPreview(
    _ path: UnsafePointer<CChar>,
    _ language: UnsafePointer<CChar>?,
    _ outputLength: UnsafeMutablePointer<Int>
) -> UnsafeMutablePointer<UInt8>?

@_silgen_name("squallz_quicklook_free")
private func freeSquallzPreview(
    _ pointer: UnsafeMutablePointer<UInt8>,
    _ length: Int
)

private enum PreviewError: Error {
    case invalidPath
    case renderFailed
}

final class PreviewProvider: QLPreviewProvider, QLPreviewingController {
    func providePreview(
        for request: QLFilePreviewRequest,
        completionHandler handler: @escaping (QLPreviewReply?, Error?) -> Void
    ) {
        let fileURL = request.fileURL
        let reply = QLPreviewReply(
            dataOfContentType: .html,
            contentSize: CGSize(width: 900, height: 700)
        ) { reply in
            reply.stringEncoding = .utf8
            return try Self.render(fileURL)
        }
        reply.title = fileURL.lastPathComponent
        handler(reply, nil)
    }

    private static func render(_ fileURL: URL) throws -> Data {
        var coordinationError: NSError?
        var result: Result<Data, Error>?
        let coordinator = NSFileCoordinator()
        coordinator.coordinate(
            readingItemAt: fileURL,
            options: .withoutChanges,
            error: &coordinationError
        ) { coordinatedURL in
            result = Result {
                try renderCoordinated(coordinatedURL)
            }
        }
        if let coordinationError {
            throw coordinationError
        }
        guard let result else {
            throw PreviewError.renderFailed
        }
        return try result.get()
    }

    private static func renderCoordinated(_ fileURL: URL) throws -> Data {
        let language = Locale.preferredLanguages.first ?? "en-US"
        return try language.withCString { languagePointer in
            try fileURL.withUnsafeFileSystemRepresentation { pathPointer in
                guard let pathPointer else {
                    throw PreviewError.invalidPath
                }
                var outputLength = 0
                guard let output = renderSquallzPreview(
                    pathPointer,
                    languagePointer,
                    &outputLength
                ), outputLength > 0 else {
                    throw PreviewError.renderFailed
                }
                defer {
                    freeSquallzPreview(output, outputLength)
                }
                return Data(bytes: output, count: outputLength)
            }
        }
    }
}
