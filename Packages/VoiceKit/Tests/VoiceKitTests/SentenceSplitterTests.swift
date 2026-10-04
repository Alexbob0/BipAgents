import Testing
@testable import VoiceKit

@Suite("Sentence splitter")
struct SentenceSplitterTests {
    /// Streams `text` in chunks of `size` characters and flushes.
    private func split(_ text: String, chunk size: Int = 3, maxLength: Int = 180) -> [String] {
        var splitter = SentenceSplitter(maxLength: maxLength)
        var out: [String] = []
        var index = text.startIndex
        while index < text.endIndex {
            let end = text.index(index, offsetBy: size, limitedBy: text.endIndex) ?? text.endIndex
            out += splitter.feed(String(text[index..<end]))
            index = end
        }
        return out + splitter.flush()
    }

    @Test func emitsSentencesAsSoonAsTheyAreComplete() {
        var splitter = SentenceSplitter()
        #expect(splitter.feed("Bonjour Sam").isEmpty)
        #expect(splitter.feed(".").isEmpty) // waits to see what follows the period
        #expect(splitter.feed(" Comment vas-tu ? Bien") == ["Bonjour Sam.", "Comment vas-tu ?"])
        #expect(splitter.flush() == ["Bien"])
    }

    @Test(arguments: [1, 2, 5, 1000])
    func splitsOnTerminators(_ chunk: Int) {
        #expect(split("Super ! Tu as dormi 7 h. C'est bien… Et demain ?", chunk: chunk)
            == ["Super !", "Tu as dormi 7 h.", "C'est bien…", "Et demain ?"])
    }

    @Test func doesNotSplitDecimalsOrTimes() {
        #expect(split("La note est 3.5 sur 5 et le coucher à 23 h 15. Parfait.") == ["La note est 3.5 sur 5 et le coucher à 23 h 15.", "Parfait."])
        #expect(split("Prends 2,5 mg vers 22.30 ce soir.") == ["Prends 2,5 mg vers 22.30 ce soir."])
    }

    @Test func doesNotSplitFrenchAbbreviations() {
        #expect(split("J'ai vu M. Dupont et Mme. Durand hier. Ils vont bien.") == ["J'ai vu M. Dupont et Mme. Durand hier.", "Ils vont bien."])
        #expect(split("Des fruits, p. ex. des pommes, sont utiles.") == ["Des fruits, p. ex. des pommes, sont utiles."])
        #expect(split("Pommes, poires, etc. sont de saison. Bon appétit.") == ["Pommes, poires, etc. sont de saison.", "Bon appétit."])
        #expect(split("Pommes, poires, etc. Ensuite le dessert.") == ["Pommes, poires, etc.", "Ensuite le dessert."])
        #expect(split("Né en 50 av. J.-C. à Rome.") == ["Né en 50 av. J.-C. à Rome."])
    }

    @Test func doesNotSplitInsideURLs() {
        #expect(split("Lis https://exemple.fr/a.b?c=1!d pour voir. Merci.", chunk: 4) == ["Lis exemple.fr pour voir.", "Merci."])
    }

    @Test func splitsOnLinesAndListItems() {
        let text = "## Plan du soir\nVoici le plan :\n- **Dîner** léger\n- Écrans coupés à 22 h\n\n1. Lecture\n2. Coucher\n"
        #expect(split(text) == ["Plan du soir.", "Voici le plan :", "Dîner léger.", "Écrans coupés à 22 h.", "1, Lecture.", "2, Coucher."])
    }

    @Test func fallsBackToLengthOnWordBoundary() {
        let words = Array(repeating: "mot", count: 30).joined(separator: " ") // 119 chars, no punctuation
        let chunks = split(words, chunk: 7, maxLength: 50)
        #expect(chunks.count == 3)
        #expect(chunks.allSatisfy { $0.count <= 50 && !$0.hasPrefix(" ") && !$0.hasSuffix(" ") })
        #expect(chunks.joined(separator: " ") == words)
    }

    @Test func prefersClauseBreaksForLongSentences() {
        let text = "Ceci est une phrase assez longue pour le test, avec une virgule et encore des mots ici"
        #expect(split(text, maxLength: 60) == ["Ceci est une phrase assez longue pour le test,", "avec une virgule et encore des mots ici"])
    }

    @Test func skipsFencedCode() {
        let text = "Lance ceci :\n```bash\nls -la. rm x.\n```\nC'est fait."
        #expect(split(text, chunk: 2) == ["Lance ceci :", "C'est fait."])
    }

    @Test func normalisesMarkdown() {
        #expect(split("**Important** : lis [la doc](https://x.fr/doc) et `make test`. Le _style_ de file_name compte.")
            == ["Important : lis la doc et make test.", "Le style de file_name compte."])
        #expect(split("> Citation inspirante.\n---\n| Jour | Heures |\n|---|---|\n| Lundi | 7 |")
            == ["Citation inspirante.", "Jour, Heures.", "Lundi, 7."])
    }

    @Test func handlesClosingQuotes() {
        #expect(split("Il a dit « Bonne nuit. » Puis il est parti.") == ["Il a dit « Bonne nuit.", "Puis il est parti."])
        #expect(split("(C'est noté.) Bonne soirée !") == ["(C'est noté.)", "Bonne soirée !"])
    }

    @Test func flushResetsState() {
        var splitter = SentenceSplitter()
        _ = splitter.feed("```\ncode")
        #expect(splitter.flush().isEmpty)
        #expect(splitter.feed("Nouveau. Texte").first == "Nouveau.")
    }
}

@Suite("Speech normalizer")
struct SpeechNormalizerTests {
    @Test(arguments: [
        ("# Titre", "Titre."),
        ("* puce", "puce."),
        ("- [x] tâche faite", "tâche faite."),
        ("![schéma](https://x/y.png) ci-dessus", "schéma ci-dessus"),
        ("Voir www.exemple.fr", "Voir www.exemple.fr"),
        ("__init__ et ~~barré~~", "init et barré"),
    ])
    func normalises(_ input: String, _ expected: String) {
        #expect(SpeechNormalizer.normalize(input) == expected)
    }

    @Test func dropsPureMarkup() {
        for markup in ["---", "***", "|---|:---:|", "``", "  "] {
            #expect(SpeechNormalizer.normalize(markup) == nil)
        }
    }
}
