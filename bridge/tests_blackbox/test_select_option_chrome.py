"""A drop-down list's choice through bip-browser-mcp (select-option-direct, 13/11) in a real Chromium, behind the
stand-in for stealth-browser-mcp (fixtures/signup_guard/stealth_cdp.py) with its `select_option`: a `value` or an
`index` set by a script declaring `const select` at the page's top level, its error never read. On the lab the same
evening, with the real server and its Chrome: the sign-up page's three birth date lists given 14, 03 and 1991 by value
answered true three times and held ["14", "", ""]; the site refused the form.

Needs a Chromium (`BIP_CHROME`) and the proxy (`BIP_BROWSER_MCP`): skipped without."""
import time

import pytest

from test_signup_guard_chrome import Session, pytestmark  # noqa: F401

LISTS = "['birthday-day', 'birthday-month', 'birthday-year'].map((i) => document.getElementById(i).value)"


@pytest.fixture(scope="module")
def browser():
    session = Session()
    yield session
    session.close()


def open_form(browser):
    browser.tool("navigate", url=f"{browser.base}/signup.html")
    browser.tool("click_element", selector="#open-register")
    time.sleep(1)


def test_the_three_birth_date_lists_of_one_page_are_all_chosen(browser):
    open_form(browser)
    said = [browser.tool("select_option", selector=f"#birthday-{part}", value=value)
            for part, value in (("day", "14"), ("month", "03"), ("year", "1991"))]
    assert browser.page(LISTS) == ["14", "03", "1991"], said


def test_a_choice_by_its_shown_text_by_its_number_or_by_its_place(browser):
    open_form(browser)
    browser.tool("select_option", selector="#birthday-month", text="mars")
    browser.tool("select_option", selector="#birthday-day", value="3")      # « 3 » for the list's « 03 »
    browser.tool("select_option", selector="#birthday-year", index=36)      # « Année », 2026 … 1991
    assert browser.page(LISTS) == ["03", "03", "1991"]
    browser.tool("select_option", selector="//select[@id='birthday-year']", value="1990")   # an XPath
    assert browser.page(LISTS)[2] == "1990"
    # the site's own handlers hear it as a person's choice
    browser.page("window.heard = []; document.getElementById('birthday-day').addEventListener('change', "
                 "(e) => window.heard.push(e.target.value))")
    browser.tool("select_option", selector="#birthday-day", value="21")
    assert browser.page("window.heard") == ["21"]


def test_a_choice_the_list_lacks_is_said_with_the_lists_choices(browser):
    open_form(browser)
    said = browser.tool("select_option", selector="#birthday-month", value="13")
    assert said.startswith("Not done:") and "Décembre" in said and "12" in said, said
    assert browser.page(LISTS)[1] == ""


def test_a_list_of_the_sites_own_is_told_to_take_two_clicks(browser):
    # signup.html's « Civilité »: a button and its choices, no <select>
    open_form(browser)
    said = browser.tool("select_option", selector="#civility", text="Madame")
    assert said.startswith("Not done:") and "click_element" in said, said
    assert browser.page("document.getElementById('civility-value').value") == ""
    said = browser.tool("select_option", selector="#nothing-here", value="1")
    assert said.startswith("Not done:"), said
