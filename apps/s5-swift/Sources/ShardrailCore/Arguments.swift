public func keyValueArguments(_ arguments: [String]) -> [String: String] {
    Dictionary(arguments.compactMap { argument in
        let parts = argument.split(separator: "=", maxSplits: 1)
        return parts.count == 2 ? (String(parts[0]), String(parts[1])) : nil
    }, uniquingKeysWith: { _, last in last })
}
