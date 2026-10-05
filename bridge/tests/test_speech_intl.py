import pytest

from bipbridge.speech_intl import prepare_for_synthesis
from bipbridge.tts import voice_language


@pytest.mark.parametrize("voice, language", [
    ("pocket:loutre", "fr"), ("5476", "fr"), ("pocket:en/loutre", "en"), ("pocket:es/ours", "es"),
    ("pocket:de/lutin", "de"), ("pocket:xx/lutin", "fr"),
])
def test_voice_language(voice, language):
    assert voice_language(voice) == language


@pytest.mark.parametrize("lang, text, spoken", [
    ("en", "Bed at 11:15, 64 °F.", "Bed at eleven fifteen, sixty-four degrees Fahrenheit."),
    ("en", "You spent $420.50 (12% less) in 2026.", "You spent four hundred and twenty dollars and fifty cents, twelve percent less, in twenty twenty-six."),
    ("en", "1,250 steps vs 1 h on the 3rd day.", "one thousand two hundred and fifty steps versus one hour on the third day."),
    ("en", "Garmin/Oura e.g. OpenAI", "Garmin or Oura for example Open AI"),
    ("es", "A las 23:15, 18 °C.", "A las veintitrés y quince, dieciocho grados."),
    ("es", "Gastaste 420,50 € el 3º día.", "Gastaste cuatrocientos veinte euros con cincuenta el tercer día."),
    ("es", "1 día, 1 semana, 1.250 pasos, 3,5 km.", "un día, una semana, mil doscientos cincuenta pasos, tres coma cinco kilómetros."),
    ("de", "Um 23:15 Uhr, 18 °C.", "Um dreiundzwanzig Uhr fünfzehn, achtzehn Grad."),
    ("de", "420,50 € am 3. Oktober 2026.", "vierhundertzwanzig Euro fünfzig am dritten Oktober zweitausendsechsundzwanzig."),
    ("de", "1 Tag, 1 Woche, 1.250 Schritte, z. B. 1999", "ein Tag, eine Woche, eintausendzweihundertfünfzig Schritte, zum Beispiel neunzehnhundertneunundneunzig"),
])
def test_prepare_for_synthesis(lang, text, spoken):
    assert prepare_for_synthesis(text, lang) == spoken
