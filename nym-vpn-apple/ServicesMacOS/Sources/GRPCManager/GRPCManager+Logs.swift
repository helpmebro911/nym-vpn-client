import GRPC
import SwiftProtobuf

extension GRPCManager {
    public func deleteLogs() async throws {
        logger.log(level: .info, "Deleting log file")

        return try await withCheckedThrowingContinuation { continuation in
            let call = client.deleteLogFile(Google_Protobuf_Empty())

            call.response.whenComplete { result in
                switch result {
                case .success(let response):
                    continuation.resume()
                case .failure(let error):
                    continuation.resume(throwing: error)
                }
            }
        }
    }
}
