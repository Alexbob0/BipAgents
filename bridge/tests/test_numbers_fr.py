import pytest

from bipbridge.numbers_fr import prepare_for_synthesis, spell_int, spell_numbers, spell_ordinal


@pytest.mark.parametrize("n, words", [
    (0, "zéro"), (1, "un"), (16, "seize"), (17, "dix-sept"), (21, "vingt et un"), (22, "vingt-deux"),
    (70, "soixante-dix"), (71, "soixante et onze"), (77, "soixante-dix-sept"), (80, "quatre-vingts"),
    (81, "quatre-vingt-un"), (91, "quatre-vingt-onze"), (99, "quatre-vingt-dix-neuf"), (100, "cent"),
    (101, "cent un"), (200, "deux cents"), (201, "deux cent un"), (1000, "mille"), (1990, "mille neuf cent quatre-vingt-dix"),
    (2026, "deux mille vingt-six"), (80000, "quatre-vingt mille"), (200000, "deux cent mille"),
    (1000000, "un million"), (2500000, "deux millions cinq cent mille"), (80000000, "quatre-vingts millions"),
])
def test_spell_int(n, words):
    assert spell_int(n) == words


@pytest.mark.parametrize("n, words", [(1, "premier"), (2, "deuxième"), (5, "cinquième"), (9, "neuvième"),
                                      (21, "vingt et unième"), (80, "quatre-vingtième"), (1000, "millième")])
def test_spell_ordinal(n, words):
    assert spell_ordinal(n) == words


@pytest.mark.parametrize("text, spoken", [
    ("Ta fréquence moyenne était de 91.", "Ta fréquence moyenne était de quatre-vingt-onze."),
    ("Rendez-vous à 14h30, puis à 9h.", "Rendez-vous à quatorze heures trente, puis à neuf heures."),
    ("Réveil à 07:05 et 1h de marche.", "Réveil à sept heures cinq et une heure de marche."),
    ("Ça fait 3,5 % de plus.", "Ça fait trois virgule cinq pour cent de plus."),
    ("Le billet coûte 1 200 € ou 1 € de frais.", "Le billet coûte mille deux cents euros ou un euro de frais."),
    ("Il fera 18 °C demain, -2° la nuit.", "Il fera dix-huit degrés demain, moins deux degrés la nuit."),
    ("Le 1er octobre, la 1re séance, le 21e jour.", "Le premier octobre, la première séance, le vingt et unième jour."),
    ("Tu as couru 10 km en 52 min, à 145 bpm.", "Tu as couru dix kilomètres en cinquante-deux minutes, à cent quarante-cinq battements par minute."),
    ("Ta VFC est de 45 ms.", "Ta VFC est de quarante-cinq millisecondes."),
    ("Appelle le 06 12 34.", "Appelle le zéro six douze trente-quatre."),
    ("Version 3.3.0 sans chiffres lus.", "Version 3.3.0 sans chiffres lus."),
    ("Entre 10-12 personnes.", "Entre dix-douze personnes."),
    ("Rien à lire ici.", "Rien à lire ici."),
])
def test_spell_numbers(text, spoken):
    assert spell_numbers(text) == spoken


@pytest.mark.parametrize("text, spoken", [
    ("Cherche sur DuckDuckGo ou YouTube.", "Cherche sur Duck Duck Go ou You Tube."),
    ("Selon OpenAI, iPhone et SNCF restent tels quels.", "Selon Open AI, iPhone et SNCF restent tels quels."),
    ("Le train (TGV) part à 9h.", "Le train, TGV, part à neuf heures."),
    ("Deux options (la seconde est moins chère).", "Deux options, la seconde est moins chère."),
    ("Prends le bus — ou marche [si beau temps].", "Prends le bus, ou marche, si beau temps."),
    ("Sommeil noté 8/10, 3/4 du trajet, 1/2 litre.", "Sommeil noté huit sur dix, trois quarts du trajet, un demi litre."),
    ("Ouvert 7j/7 et 24h/24.", "Ouvert sept jours sur sept et vingt-quatre heures sur vingt-quatre."),
    ("Rendez-vous le 04/10/2026, rappel le 01/11.", "Rendez-vous le quatre octobre deux mille vingt-six, rappel le premier novembre."),
    ("Données Garmin/Oura, billet aller/retour, A/R, et/ou à 50 km/h.",
     "Données Garmin ou Oura, billet aller-retour, aller-retour, et ou à cinquante kilomètres heure."),
])
def test_prepare_for_synthesis(text, spoken):
    assert prepare_for_synthesis(text) == spoken


@pytest.mark.parametrize("text, spoken", [
    ("Bordeaux St-Jean → Paris Montparnasse le 03/02/2027 : 19 €.",
     "Bordeaux Saint-Jean vers Paris Montparnasse le trois février deux mille vingt-sept : dix-neuf euros."),
    ("RDV avec Mme Martin et M. Durand, Dr Lopez absent.", "rendez-vous avec Madame Martin et Monsieur Durand, Docteur Lopez absent."),
    ("Comptez env. 2 h, p. ex. le matin, c.-à-d. tôt.", "Comptez environ deux heures, par exemple le matin, c'est-à-dire tôt."),
    ("Pommes, poires, etc. Puis le dessert.", "Pommes, poires, et cetera, Puis le dessert."),
    ("Ligne n°4, Sainte-Anne & Ste Marie, ≈ 30 min, ≥ 5 km.",
     "Ligne numéro quatre, Sainte-Anne et Sainte Marie, environ trente minutes, au moins cinq kilomètres."),
    ("Wellness + Vie, 3 × 10 reps, PSG vs OM.", "ouèlnesse plus Vie, trois fois dix reps, PSG contre OM."),
])
def test_lexicon(text, spoken):
    assert prepare_for_synthesis(text) == spoken


@pytest.mark.parametrize("text, spoken", [
    ("Ça prend ~15 min, soit ~ une heure en tout, ou 2~3 jours.",
     "Ça prend environ quinze minutes, soit environ une heure en tout, ou deux à trois jours."),
    ("Les runs Hermes et le cron du matin.", "Les reunes Hermes et le crone du matin."),
    ("Lance un Live avec Wellness, puis check tes emails.", "Lance un laïve avec ouèlnesse, puis tchèque tes imèles."),
    ("Rien d'anglais dans « il court » ni « livret ».", "Rien d'anglais dans « il court » ni « livret »."),
])
def test_tilde_and_english_words(text, spoken):
    assert prepare_for_synthesis(text) == spoken
