🇬🇧 English · [🇫🇷 Français](privacy-compare.fr.md)

# Where your conversations go: BipAgents and the consumer assistants

**Checked on 22 October 2026.** This page compares what the **consumer** offers (free or paid, for individuals) of a
few AI assistants do **by default**, from their own pages, with what BipAgents does, from its
[privacy policy](legal/privacy.en.md) and [what the apps send](app-data.md). These companies' business or API offers
often have other rules. The rules change: each line points to its source and its date. If you spot a mistake, write to
support@kerfystudio.com.

## At a glance

| | Where your conversations are kept | AI training, by default | How long | Read by people |
|---|---|---|---|---|
| **ChatGPT** (OpenAI) | On OpenAI's servers, in your history until you delete them | Yes, unless you turn off "Improve the model for everyone" [1] | Deleted: erased within 30 days, with exceptions (security, legal obligations) [2] | Not covered here |
| **Gemini** (Google) | With your Google account ("Activity") | Yes while "Keep Activity" is on, which is the default [3] | Auto-deleted after 18 months by default; 72 hours with activity off [3] | Yes, a share of conversations, kept up to 3 years [3] |
| **Claude** (Anthropic) | On Anthropic's servers | Only if "Help improve Claude" is on; the switch was pre-set to on when the choice was asked in 2025 [4][5] | Up to 5 years with the switch on, otherwise 30 days after deletion [6] | Not covered here |
| **Meta AI** | On Meta's servers | Meta uses interactions with its AIs to improve its products [7] | Not found | Yes, by people and automated tools [7] |
| **Copilot** (Microsoft, new app since 18 August 2026) | On Microsoft's servers | Not for its "foundation models", per Microsoft [8] | Not stated for the new app | Not covered here |
| **BipAgents** | On **your place** (an account of your own on a server, separate from the others) or on your own computer | **No**: Kerfy Studio trains no AI with your data [9] | As long as you keep your place; erased 30 days after the account is deleted [10] | Kerfy Studio does not read your place (see the limits below) [11] |

## What changed recently

- **ChatGPT and the US courts.** In May 2025, in The New York Times' case against OpenAI, a court ordered OpenAI to keep
  every conversation, even deleted ones [12]. The order ended for new conversations on 26 September 2025; what was kept
  in the meantime stays kept [13]. Conversations from the European Economic Area, Switzerland and the UK were outside
  it, according to OpenAI [14]. In January 2026 a judge upheld handing 20 million de-identified conversations to the
  plaintiffs [15].
- **Meta AI and ads.** Since 16 December 2025, Meta uses interactions with Meta AI to personalise content and ads,
  except sensitive topics [16]. According to the press there is no switch to refuse it, and the EU, the UK and South
  Korea were left out at first [17].
- **Claude.** Since autumn 2025, Free, Pro and Max conversations may be used for training if the person agrees [4].
  Memory is on by default [18].
- **Copilot.** Microsoft's new app (18 August 2026) says it does not train its foundation models on your
  conversations; the old one did by default, with a switch to turn it off, and kept conversations 18 months [8][19].

## Memory

All these assistants now have a memory that keeps things about you from one conversation to the next: ChatGPT [20],
Gemini [3], Claude (on by default) [18], Meta AI [21], Copilot [22]. The company keeps it. In BipAgents, your agents'
memory is a file on your place, which you can read and correct in the app.

## What BipAgents does not promise

- **On the all-included offer, Kerfy Studio rents the server** (OVHcloud or Hetzner, in Europe). Technically, the host
  and Kerfy Studio could access your place. Kerfy Studio's software does not read it, access to the company's host
  accounts is restricted to the company, and access would only happen under a legal obligation [11]. OVHcloud also
  takes a daily image of its servers, which Kerfy Studio does not encrypt [23].
- **The AI reads what your agents send it**, in order to answer. On the offer this goes through EUrouter (Amsterdam),
  which says it keeps neither the messages nor the answers but keeps their metadata up to 12 months, then through the
  provider that runs the chosen model [24].
- **"Report this reply"** sends the reply's text to Kerfy Studio, when you send it [25].
- What the apps guarantee is described in [what the apps send](app-data.md), with its limits.

With your own computer or server, none of this goes through Kerfy Studio: only notifications go through its relay,
sealed [26].

## Sources

Accessed on 22 October 2026 unless another date is given.

1. OpenAI, *Data Controls FAQ* — https://help.openai.com/en/articles/7730893-data-controls-faq
2. OpenAI, *How to delete your account* — https://help.openai.com/en/articles/6378407-how-to-delete-your-account
3. Google, *Gemini Apps Privacy Hub* (updated 19 October 2026) — https://support.google.com/gemini/answer/13594961
4. Anthropic, *Updates to our consumer terms* (28 August 2025) — https://www.anthropic.com/news/updates-to-our-consumer-terms
5. TechCrunch, 28 August 2025 — https://techcrunch.com/2025/08/28/anthropic-users-face-a-new-choice-opt-out-or-share-your-data-for-ai-training/
6. Anthropic, *How long do you store my data?* (1 July 2026) — https://privacy.claude.com/en/articles/10023548-how-long-do-you-store-my-data
7. Meta, *Privacy matters: Meta's generative AI features* (September 2023) — https://about.fb.com/news/2023/09/privacy-matters-metas-generative-ai-features/
8. Microsoft, *Copilot activity history* — https://support.microsoft.com/en-us/privacy/microsoft-copilot/activity-history ; *overview* — https://support.microsoft.com/en-us/privacy/microsoft-copilot/overview
9. [BipAgents privacy policy](legal/privacy.en.md), section 6
10. Same text, section 5.2
11. Same text, section 5.1
12. CyberInsider, May 2025 — https://cyberinsider.com/us-court-orders-openai-to-preserve-all-chatgpt-logs-indefinitely/
13. Engadget, 11 October 2025 — https://www.engadget.com/ai/openai-no-longer-has-to-preserve-all-of-its-chatgpt-data-with-some-exceptions-192422093.html
14. OpenAI, *How we're responding to The New York Times' data demands* (updated 22 October 2025) — https://openai.com/index/response-to-nyt-data-demands/
15. ABA Journal, 8 January 2026 — https://www.abajournal.com/news/article/chatgpt-creator-must-turn-over-20m-chat-logs-in-copyright-litigation-federal-judge-says
16. Meta, *Improving your recommendations with AI at Meta* (1 October 2025) — https://about.fb.com/news/2025/10/improving-your-recommendations-apps-ai-meta/
17. Business Today, 3 October 2025 — https://www.businesstoday.in/technology/news/story/meta-to-use-ai-chat-data-for-ads-and-posts-from-december-16-and-you-cant-say-no-496635-2025-10-03
18. Anthropic, *Use Claude's chat search and memory* — https://support.claude.com/en/articles/11817273-use-claude-s-chat-search-and-memory-to-build-on-previous-context
19. Microsoft, *Privacy FAQ for Microsoft Copilot* (old app) — https://support.microsoft.com/en-us/microsoft-copilot/privacy-faq-for-microsoft-copilot
20. OpenAI, *Memory in ChatGPT* — https://help.openai.com/en/articles/8590148-memory-in-chatgpt
21. Meta, *Building toward a smarter, more personalized assistant* (27 January 2025) — https://about.fb.com/news/2025/01/building-toward-a-smarter-more-personalized-assistant/
22. Microsoft, *Copilot privacy controls* — https://support.microsoft.com/en-us/privacy/microsoft-copilot/privacy-controls
23. [BipAgents privacy policy](legal/privacy.en.md), section 5.5
24. Same text, section 5.4
25. [What the apps send](app-data.md), section 3
26. [BipAgents privacy policy](legal/privacy.en.md), section 4.1
