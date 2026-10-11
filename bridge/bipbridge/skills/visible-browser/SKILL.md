---
name: visible-browser
description: "Browse the web with your own visible browser: a real Chrome on your desktop, which the person can watch and take over from the BipAgents app. Use its `stealth` tools for every website, including sites that block automated browsers."
version: 1.1.0
author: BipAgents
license: MIT
metadata:
  hermes:
    tags: [browser, web, desktop, vault]
---

# Your visible browser (`stealth` tools)

You have a real Google Chrome (Chromium on some machines) on a desktop of your own. Its tools are named
`stealth_*` or `mcp__stealth__*` (spawn_browser, navigate, click_element, type_text, get_page_content, take_screenshot,
list_tabs, execute_script, close_instance…). The person can watch this browser live in the app (« Bureau ») and take
over the mouse and keyboard.

## 1. Start it once per task
Call `spawn_browser` with no arguments: the profile (cookies and logins kept between tasks), the window, the language
and the time zone are already set. Keep the `instance_id` it returns for every later call. Never start a second
browser while one runs: reuse the instance (`list_instances` tells you if one is already open).

## 2. Browse
- After opening a page of a site that checks for robots, wait 5 to 10 seconds before reading it.
- Read pages as text (`get_page_content`) rather than with screenshots when you can.
- Cookie banners: choose the option that refuses non-essential cookies.

## 3. A captcha, a slider or « verify you are human »
Do not try to solve it. Ask the person: « A check needs you: open my Bureau in the BipAgents app, take over, complete
it, then tell me "ok". » Wait for their answer, then read the page again. Do not reload in a loop.

## 4. Logging in with the vault
The vault fills a password without you ever seeing it, on the exact site it was saved for:
1. open the login page with your browser (`navigate`), type the identifier yourself if it asks for it first;
2. call `browser_vault_list`, then `browser_vault_fill` with the item for this site, on this same page;
3. continue with your browser (submit, next page).
The vault tools act on the same Chrome as your `stealth` tools. If they answer that no browser is running, start
yours with `spawn_browser` first.

## When Hermes' browser tools find no browser
Hermes' own browser tools (`browser_exec`, `browser_navigate`…) and the vault drive this same Chrome: they have no
browser of their own. At the **first** error saying the browser is unreachable (`BU_CDP_URL … unreachable`,
`Connection refused` on 127.0.0.1), do not call the same tool again: call `spawn_browser` first, then redo the same
call. Never fall back to a web search for something the person asked you to do on a website. Never read back a password field (no script on it), never write a password in a
message.

## 5. Payments and anything that cannot be undone
Never pay, order, send or delete on a website without the person's explicit go for that exact action: stop on the
confirmation page and tell them what is about to happen.

## 6. When you are done
Leave the browser open if the person is likely to continue; otherwise `close_instance`. It also closes by itself after
30 minutes without use.
