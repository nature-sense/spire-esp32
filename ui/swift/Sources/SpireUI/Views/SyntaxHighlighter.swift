import SwiftUI

/// Pure-Swift syntax highlighter (no C dependencies -> cannot crash).
/// Uses `AttributedString` with regex-based token colours.
///
/// **C/C++ only.** This is a C++/ESP-IDF project; the files the explorer shows are C/C++
/// source or plain text (`CMakeLists.txt`, `sdkconfig`). Anything this highlighter does not
/// know is `.plain` — no colour, never a wrong one.
enum SyntaxLanguage {
    case cpp, plain

    /// Detect language from a file path/extension (case-insensitive).
    static func detect(from path: String) -> SyntaxLanguage {
        switch (path as NSString).pathExtension.lowercased() {
        case "c", "h", "cc", "cpp", "cxx", "hh", "hpp", "hxx", "inl", "ipp":
            return .cpp
        default:
            return .plain
        }
    }

    /// Reserved words.
    fileprivate var keywords: Set<String> {
        switch self {
        case .cpp:
            return ["alignas","alignof","and","asm","auto","bool","break","case","catch",
                    "char","class","const","constexpr","const_cast","continue","decltype",
                    "default","delete","do","double","dynamic_cast","else","enum","explicit",
                    "export","extern","false","float","for","friend","goto","if","inline",
                    "int","long","mutable","namespace","new","noexcept","nullptr","operator",
                    "private","protected","public","register","reinterpret_cast","return",
                    "short","signed","sizeof","static","static_assert","static_cast","struct",
                    "switch","template","this","throw","true","try","typedef","typeid",
                    "typename","union","unsigned","using","virtual","void","volatile",
                    "wchar_t","while"]
        case .plain:
            return []
        }
    }

    /// Line-comment marker.
    fileprivate var lineComments: [String] {
        switch self {
        case .cpp: return ["//"]
        case .plain: return []
        }
    }
}

enum SyntaxHighlighter {
    static func highlight(_ code: String, language: SyntaxLanguage) -> AttributedString {
        var attr = AttributedString(code)
        // Work in UTF-16 offsets (what NSRange/NSRegularExpression use).
        let ns = code as NSString
        let lines = code.components(separatedBy: "\n")
        var location = 0

        for line in lines {
            let lineLen = (line as NSString).length

            // Comment colour (green) — colour the span after the marker, only if
            // the marker appears outside a string literal.
            if let cm = language.lineComments.first {
                let cmNS = cm as NSString
                let text = line as NSString
                let marker = text.range(of: cm)

                func isInString(_ loc: Int) -> Bool {
                    let prefix = text.substring(to: loc) as NSString
                    let quotes = prefix.range(of: "\"", options: [.backwards]).location
                    let single = prefix.range(of: "'", options: [.backwards]).location
                    // Naive but suitable for line-based highlighting: a quote
                    // before the marker with no unpaired counterpart on the line.
                    return max(quotes, single) != NSNotFound
                }

                if marker.location != NSNotFound, !isInString(marker.location) {
                    let commentLength = lineLen - (marker.location + cmNS.length)
                    if commentLength > 0 {
                        let nsRng = NSRange(location: location + marker.location + cmNS.length,
                                            length: commentLength)
                        if let r = Range(nsRng, in: attr) {
                            attr[r].foregroundColor = .green
                        }
                    }
                }
            }

            // Keywords (blue) + string literals (red) via regex over this line.
            let regex = try? NSRegularExpression(
                pattern: "\\b([A-Za-z_][A-Za-z0-9_]*)\\b|(\"[^\"]*\"|'[^']*')"
            )
            for match in regex?.matches(in: line, range: NSRange(location: 0, length: lineLen)) ?? [] {
                let wordRange = match.range(at: 1)
                if wordRange.location != NSNotFound {
                    let word = (line as NSString).substring(with: wordRange)
                    if language.keywords.contains(word) {
                        let nsRng = NSRange(location: location + wordRange.location, length: wordRange.length)
                        if let r = Range(nsRng, in: attr) {
                            attr[r].foregroundColor = .blue
                        }
                    }
                }
                let strRange = match.range(at: 2)
                if strRange.location != NSNotFound {
                    let nsRng = NSRange(location: location + strRange.location, length: strRange.length)
                    if let r = Range(nsRng, in: attr) {
                        attr[r].foregroundColor = .red
                    }
                }
            }

            // Advance past this line and its trailing newline (if any).
            location += lineLen + 1
        }
        return attr
    }
}