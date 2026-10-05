import SwiftUI

/// A mascot you can play with:
/// - tap → a random reaction (hop, wink, spin, surprise, heart) with a little speech bubble;
/// - quick taps or rubbing back and forth → tickles: giggles and shakes;
/// - slow stroke → petting: blissful face, blush, floating hearts;
/// - press and hold → squashed under the finger, then « Boing ! » on release;
/// - drag → follows the finger elastically and looks at it;
/// - too many pokes → dizzy, then sulks for a moment.
struct InteractiveMascot: View {
    var appearance: AgentAppearance
    /// Mood when nobody is playing (listening, thinking… while working).
    var baseMood: MascotMood = .happy
    var size: CGFloat
    var speakingLevel: Double?
    /// Where speech bubbles appear: above the head (centred Bips) or to the left (Bips on a card's edge).
    var bubbleEdge: BubbleEdge = .top

    enum BubbleEdge { case top, leading }

    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    @State private var reactionMood: MascotMood?
    @State private var scale = CGSize(width: 1, height: 1)
    @State private var offset: CGSize = .zero
    @State private var rotation: Angle = .zero
    @State private var gaze: CGVector = .zero
    @State private var blush: Double = 0
    @State private var bubble: String?
    @State private var particles: [Particle] = []
    @State private var tapTimes: [Date] = []
    @State private var pokeTimes: [Date] = []
    @State private var squeezed = false
    @State private var rub = RubTracker()
    @State private var haptic = 0
    @State private var recentlyDragged = false
    @State private var handledOnTouchDown = false
    /// Dizzy then grumpy: other reactions wait until it's over.
    @State private var sulking = false
    @State private var moodTask: Task<Void, Never>?
    @State private var bubbleTask: Task<Void, Never>?

    var body: some View {
        MascotView(appearance: appearance, mood: reactionMood ?? baseMood, speakingLevel: speakingLevel, gaze: gaze, blush: blush)
            .frame(width: size, height: size)
            .scaleEffect(x: scale.width, y: scale.height, anchor: .bottom)
            .rotationEffect(rotation, anchor: .bottom)
            .offset(offset)
            .overlay { particleLayer }
            .overlay(alignment: bubbleEdge == .top ? .top : .leading) {
                switch bubbleEdge {
                case .top:
                    // Zero-height frame on top: the bubble's bottom sits just above the head.
                    bubbleView.frame(height: 0, alignment: .bottom).offset(y: size * 0.02)
                case .leading:
                    // Zero-width frame on the leading edge: the bubble's trailing edge sits there, text runs left.
                    bubbleView.frame(width: 0, alignment: .trailing).offset(x: size * 0.08, y: -size * 0.12)
                }
            }
            .zIndex(1)
            .contentShape(.circle)
            .onTapGesture {
                // A drag that ends where it began also reads as a tap: ignore it.
                guard !recentlyDragged else { return }
                tapped()
            }
            .onLongPressGesture(minimumDuration: 0.35, maximumDistance: 24) {
                squeezed = true
                haptic += 1
            } onPressingChanged: { pressing in
                if pressing { touchedDown() }
                pressing ? squash() : release()
            }
            .simultaneousGesture(dragGesture)
            .sensoryFeedback(.impact(flexibility: .soft, intensity: 0.8), trigger: haptic)
            .accessibilityElement()
            .accessibilityLabel("Bip, la mascotte")
            .accessibilityHint("Touche-la pour la faire réagir")
            .accessibilityAddTraits(.isButton)
            .accessibilityAction { tapped() }
    }

    // MARK: Taps

    /// Counts every finger landing on the Bip — even jittery ones the tap recognizer drops — so tickling
    /// (3 in 1.2 s) and pestering (8 in 6 s) are noticed.
    private func touchedDown() {
        let now = Date.now
        tapTimes = tapTimes.filter { now.timeIntervalSince($0) < 1.2 } + [now]
        pokeTimes = pokeTimes.filter { now.timeIntervalSince($0) < 6 } + [now]
        guard !sulking else { return }
        if pokeTimes.count >= 8 {
            pokeTimes = []
            tapTimes = []
            handledOnTouchDown = true
            annoyed()
        } else if tapTimes.count >= 3 {
            tapTimes = []
            handledOnTouchDown = true
            giggle()
        }
    }

    private func tapped() {
        haptic += 1
        if handledOnTouchDown {
            handledOnTouchDown = false
            return
        }
        guard !sulking else { return }
        [hop, wink, spin, surprised, heart].randomElement()!()
    }

    private func hop() {
        show(.happy, for: 0.8)
        say([String(localized: "Bip !"), String(localized: "Coucou !"), String(localized: "Oui ?"), String(localized: "Hop !")].randomElement()!)
        jump(height: size * 0.22)
    }

    private func wink() {
        show(.winking, for: 1.1)
        say([String(localized: "Hé hé"), String(localized: "Bien joué"), String(localized: "Toujours là !")].randomElement()!)
        pop(1.08)
    }

    private func spin() {
        show(.giggling, for: 0.9)
        say(String(localized: "Wiii !"))
        animate(.spring(duration: 0.7, bounce: 0.35)) { rotation += .degrees(360) }
    }

    private func surprised() {
        show(.surprised, for: 1.0)
        say([String(localized: "Oh !"), String(localized: "Ah ?!"), String(localized: "Quoi ?")].randomElement()!)
        pop(1.14)
    }

    private func heart() {
        show(.content, for: 1.4)
        say(String(localized: "Merci ♡"))
        burst(.heart, count: 3)
        blushUp()
    }

    private func giggle() {
        show(.giggling, for: 1.8)
        say([String(localized: "Hihi !"), String(localized: "Ça chatouille !"), String(localized: "Arrête, hihi !"), String(localized: "Hahaha !")].randomElement()!)
        burst(.sparkle, count: 4)
        shake(times: 8, angle: 9)
    }

    private func annoyed() {
        sulking = true
        show(.dizzy, for: 1.8)
        say(String(localized: "Tout tourne…"))
        shake(times: 5, angle: 14)
        moodTask?.cancel()
        moodTask = Task {
            try? await Task.sleep(for: .seconds(1.8))
            guard !Task.isCancelled else { return }
            reactionMood = .grumpy
            say(String(localized: "Hé ! Doucement…"))
            try? await Task.sleep(for: .seconds(2.4))
            sulking = false
            guard !Task.isCancelled else { return }
            reactionMood = nil
        }
    }

    // MARK: Squeeze

    private func squash() {
        animate(.spring(duration: 0.25, bounce: 0.2)) { scale = CGSize(width: 1.14, height: 0.82) }
    }

    private func release() {
        if squeezed {
            squeezed = false
            show(.surprised, for: 0.9)
            say(String(localized: "Boing !"))
            haptic += 1
            jump(height: size * 0.42)
        } else {
            animate(.spring(duration: 0.3, bounce: 0.5)) { scale = CGSize(width: 1, height: 1) }
        }
    }

    // MARK: Drag: follow, tickle, pet

    private var dragGesture: some Gesture {
        DragGesture(minimumDistance: 6)
            .onChanged { value in
                let t = value.translation
                animate(.interactiveSpring) {
                    offset = CGSize(width: rubberBand(t.width), height: rubberBand(t.height))
                    gaze = CGVector(dx: t.width / 60, dy: t.height / 60)
                }
                recentlyDragged = true
                // Moving the finger means stroking or tickling, not squeezing.
                if hypot(t.width, t.height) > 15, squeezed || scale != CGSize(width: 1, height: 1) {
                    squeezed = false
                    animate(.spring(duration: 0.3, bounce: 0.4)) { scale = CGSize(width: 1, height: 1) }
                }
                switch rub.add(value.location, at: value.time) {
                case .tickle where !sulking: giggle()
                case .pet where !sulking: pet()
                default: break
                }
            }
            .onEnded { _ in
                rub = RubTracker()
                Task {
                    try? await Task.sleep(for: .milliseconds(350))
                    recentlyDragged = false
                }
                animate(.spring(duration: 0.45, bounce: 0.45)) {
                    offset = .zero
                    gaze = .zero
                }
            }
    }

    private func pet() {
        show(.content, for: 2)
        say([String(localized: "Mmmh…"), String(localized: "Encore…"), String(localized: "Ronron")].randomElement()!)
        burst(.heart, count: 2)
        blushUp()
        haptic += 1
    }

    // MARK: Building blocks

    private func show(_ mood: MascotMood, for seconds: Double) {
        moodTask?.cancel()
        reactionMood = mood
        moodTask = Task {
            try? await Task.sleep(for: .seconds(seconds))
            if !Task.isCancelled { reactionMood = nil }
        }
    }

    private func say(_ text: String) {
        bubbleTask?.cancel()
        withAnimation(.spring(duration: 0.3, bounce: 0.4)) { bubble = text }
        BipBabble.shared.say(text, appearance: appearance, mood: reactionMood)
        bubbleTask = Task {
            try? await Task.sleep(for: .seconds(1.5))
            guard !Task.isCancelled else { return }
            withAnimation(.easeOut(duration: 0.25)) { bubble = nil }
        }
    }

    private func jump(height: CGFloat) {
        animate(.spring(duration: 0.22, bounce: 0.1)) {
            offset.height = -height
            scale = CGSize(width: 0.88, height: 1.14)
        }
        Task {
            try? await Task.sleep(for: .milliseconds(220))
            animate(.spring(duration: 0.5, bounce: 0.55)) {
                offset.height = 0
                scale = CGSize(width: 1, height: 1)
            }
        }
    }

    private func pop(_ factor: CGFloat) {
        animate(.spring(duration: 0.2, bounce: 0.3)) { scale = CGSize(width: factor, height: factor) }
        Task {
            try? await Task.sleep(for: .milliseconds(200))
            animate(.spring(duration: 0.4, bounce: 0.5)) { scale = CGSize(width: 1, height: 1) }
        }
    }

    private func shake(times: Int, angle: Double) {
        Task {
            for i in 0..<times {
                animate(.easeInOut(duration: 0.07)) { rotation = .degrees(i.isMultiple(of: 2) ? angle : -angle) }
                if i.isMultiple(of: 2) { haptic += 1 }
                try? await Task.sleep(for: .milliseconds(70))
            }
            animate(.spring(duration: 0.3, bounce: 0.4)) { rotation = .zero }
        }
    }

    private func blushUp() {
        withAnimation(.easeOut(duration: 0.3)) { blush = 1 }
        Task {
            try? await Task.sleep(for: .seconds(1.6))
            withAnimation(.easeInOut(duration: 0.8)) { blush = 0 }
        }
    }

    private func burst(_ kind: Particle.Kind, count: Int) {
        let new = (0..<count).map { _ in Particle(kind: kind, x: .random(in: -0.35...0.35) * size, delay: .random(in: 0...0.25)) }
        particles += new
        Task {
            try? await Task.sleep(for: .seconds(1.6))
            particles.removeAll { p in new.contains { $0.id == p.id } }
        }
    }

    /// Respects Reduce Motion: no movement, the faces still react.
    private func animate(_ animation: Animation, _ change: () -> Void) {
        if reduceMotion {
            var transaction = Transaction()
            transaction.disablesAnimations = true
            withTransaction(transaction, change)
        } else {
            withAnimation(animation, change)
        }
    }

    private func rubberBand(_ distance: CGFloat) -> CGFloat {
        let limit = size * 0.3
        return limit * (1 - 1 / (abs(distance) / limit * 0.55 + 1)) * (distance < 0 ? -1 : 1)
    }

    // MARK: Overlays

    @ViewBuilder
    private var bubbleView: some View {
        if let bubble {
            Text(bubble)
                .font(Theme.body(max(12, size * 0.13), weight: .black))
                .foregroundStyle(appearance.palette.deep)
                .lineLimit(1)
                .fixedSize() // phrases stay short (≤ 15 characters); placement keeps them on screen
                .padding(.horizontal, 10)
                .padding(.vertical, 5)
                .background(Theme.card, in: .capsule)
                .shadow(color: .black.opacity(0.12), radius: 6, y: 3)
                .transition(.scale(scale: 0.4, anchor: bubbleEdge == .top ? .bottom : .trailing).combined(with: .opacity))
                .allowsHitTesting(false)
        }
    }

    private var particleLayer: some View {
        ZStack {
            ForEach(particles) { particle in
                FloatingParticle(particle: particle, size: size, color: particle.kind == .heart ? Color(hex: 0xFF5C86) : Color(hex: 0xF6B826))
            }
        }
        .allowsHitTesting(false)
    }
}

struct Particle: Identifiable, Equatable {
    enum Kind { case heart, sparkle }
    let id = UUID()
    var kind: Kind
    var x: CGFloat
    var delay: Double
}

private struct FloatingParticle: View {
    var particle: Particle
    var size: CGFloat
    var color: Color
    @State private var rising = false

    var body: some View {
        Image(systemName: particle.kind == .heart ? "heart.fill" : "sparkle")
            .font(.system(size: max(12, size * 0.16), weight: .bold))
            .foregroundStyle(color)
            .offset(x: particle.x, y: rising ? -size * 0.75 : -size * 0.25)
            .scaleEffect(rising ? 1.15 : 0.4)
            .opacity(rising ? 0 : 1)
            .onAppear {
                withAnimation(.easeOut(duration: 1.2).delay(particle.delay)) { rising = true }
            }
    }
}

/// Classifies a drag on the mascot: fast back-and-forth = tickle; a long, slow stroke = petting.
struct RubTracker {
    enum Gesture { case tickle, pet }

    private var last: CGPoint?
    private var direction: CGFloat = 0
    private var reversals: [Date] = []
    private var travelled: CGFloat = 0
    private var started: Date?
    private var fired = false

    mutating func add(_ point: CGPoint, at time: Date) -> Gesture? {
        defer { last = point }
        if started == nil { started = time }
        guard let last, !fired else { return nil }
        let dx = point.x - last.x
        travelled += hypot(dx, point.y - last.y)
        if abs(dx) > 3 {
            let sign: CGFloat = dx > 0 ? 1 : -1
            if direction != 0, sign != direction { reversals.append(time) }
            direction = sign
        }
        reversals = reversals.filter { time.timeIntervalSince($0) < 0.9 }
        if reversals.count >= 4 {
            fired = true
            return .tickle
        }
        if let started, time.timeIntervalSince(started) > 0.6, travelled > 45, reversals.count <= 1 {
            fired = true
            return .pet
        }
        return nil
    }
}
