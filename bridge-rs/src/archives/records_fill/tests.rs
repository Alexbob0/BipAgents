//! Synthetic archives only (no real mail), a fake model answering canned JSON.

use super::*;
use crate::archives::extract::Doc;
use crate::config::AgentConfig;

fn doc(title: &str, sender: &str, date: &str, text: &str) -> Doc {
    Doc { title: title.into(), sender: sender.into(), date: date.into(), text: text.into() }
}

fn index_with(dir: &Path, docs: &[(&str, Doc)]) -> Index {
    let index = Index::open(&dir.join("i.sqlite")).unwrap();
    for (loc, d) in docs {
        index.insert(loc, loc, 1, 1, "note", d, 1).unwrap();
    }
    index
}

#[test]
fn the_pre_filter_keeps_likely_records_and_leaves_adverts() {
    let dir = tempfile::tempdir().unwrap();
    let index = index_with(
        dir.path(),
        &[
            ("order.md", doc("Votre commande n°123", "Boutique", "2024-05-02", "Merci, 1 299 €")),
            ("note.md", doc("Idées de week-end", "", "2024-06-01", "Aller voir la mer, lire")),
            ("promo.md", doc("Newsletter : soldes d'été", "Shop", "2024-07-01", "Tout à -50 %")),
            (
                "phrase.md",
                doc("Bonjour", "Garage", "2023-01-02", "Votre numéro de commande est le X12, total TTC 89 €"),
            ),
            (
                "promo-order.md",
                doc("Promo : votre commande est partie", "Shop", "2022-03-03", "Numéro de commande : AB12"),
            ),
            ("insurance.md", doc("Attestation d'assurance habitation", "Assureur", "2021-01-05", "Contrat")),
            ("undated.md", doc("Facture électricité", "Énergie", "", "Montant total 42 €")),
        ],
    );
    let kept: Vec<String> = candidates(&index).unwrap().into_iter().map(|r| r.hit.loc).collect();
    // the oldest first, those without a date last; the advert left out, the advert with an order number kept
    assert_eq!(kept, vec!["insurance.md", "promo-order.md", "phrase.md", "order.md", "undated.md"]);
    let all: Vec<Row> = (0..10)
        .map(|i| Row { id: i, hit: index.rows().unwrap()[0].hit.clone(), category: "other".into(), size: 0, mtime: 0 })
        .collect();
    let picked: Vec<i64> = spread(all.clone(), 3).into_iter().map(|r| r.id).collect();
    assert_eq!(picked, vec![1, 5, 8]);
    assert_eq!(spread(all, 20).len(), 10);
}

const TEXT: &str = "Commande du 2 mai 2024 chez Boutique Cycles : vélo électrique, 1 299,00 €. \
Contrat habitation n° HAB-2024-778, Assureur Durand. Réservation ABC123 hôtel à Lisbonne.";

#[test]
fn records_are_checked_against_their_document() {
    let ok = |v: Value| check(&v, TEXT, "2024-05-02");
    let err = |v: Value| match check(&v, TEXT, "2024-05-02") {
        Ok(_) => panic!("accepted"),
        Err(why) => why,
    };
    let (kind, fields) = ok(json!({"type": "purchase", "fields": {"date": "2024-05-02", "item": "Vélo électrique",
        "store": "Boutique Cycles", "price": "1 299,00 €"}}))
    .unwrap();
    assert_eq!(kind.id, "purchase");
    assert_eq!(fields["price"], json!("1 299,00 €"));
    // an invented field rejects the whole record
    assert!(err(json!({"type": "purchase", "fields": {"item": "Vélo", "colour": "rouge"}})).contains("colour"));
    assert!(ok(json!({"type": "purchase", "fields": {"item": "Vélo", "source": "x"}})).is_err());
    assert!(err(json!({"type": "receipt", "fields": {"item": "Vélo"}})).contains("unknown type"));
    // values that must come from the document
    assert!(ok(json!({"type": "contract", "fields": {"what": "habitation", "number": "HAB-2024-999"}})).is_err());
    let (_, c) = ok(json!({"type": "contract", "fields": {"what": "habitation", "number": "hab 2024 778"}})).unwrap();
    assert_eq!(c["number"], json!("hab 2024 778"));
    assert!(err(json!({"type": "purchase", "fields": {"item": "Vélo", "price": "999 €"}})).contains("price"));
    assert!(err(json!({"type": "purchase", "fields": {"item": "Trottinette"}})).contains("item"));
    assert!(ok(json!({"type": "booking", "fields": {"what": "hôtel", "reference": "ZZZ999"}})).is_err());
    // a card number is never kept; a date whose year isn't in the document is dropped
    assert!(ok(json!({"type": "booking", "fields": {"what": "hôtel", "reference": "4970 1012 3456 7890"}})).is_err());
    let (_, p) = ok(
        json!({"type": "purchase", "fields": {"item": "vélo", "date": "2019-01-01", "store": "4970 1012 3456 7890"}}),
    )
    .unwrap();
    assert!(!p.contains_key("date") && !p.contains_key("store"));
    // incomplete records
    assert!(ok(json!({"type": "purchase", "fields": {"store": "Boutique Cycles"}})).is_err());
    assert!(ok(json!({"type": "purchase", "fields": {"item": ["vélo"]}})).is_err());
    assert!(sensitive("FR76 3000 6000 0112 3456 7890 189") && !sensitive("HAB-2024-778"));
}

fn doc_in(title: &str, text: &str) -> DocIn {
    DocIn {
        row: Row {
            id: 1,
            hit: crate::archives::index::Hit {
                key: "0123456789abcdef".into(),
                loc: "a.md".into(),
                file: "a.md".into(),
                kind: "note".into(),
                title: title.into(),
                sender: String::new(),
                date: "2024-05-02".into(),
                preview: String::new(),
            },
            category: "purchase".into(),
            size: 1,
            mtime: 1,
        },
        text: text.into(),
        hash: hash_of(text),
    }
}

#[test]
fn answers_are_parsed_strictly() {
    let docs = vec![doc_in("Commande", TEXT)];
    assert!(parse_answer("Désolé, je ne peux pas.", &docs).is_err());
    assert!(parse_answer("{\"records\": []}", &docs).is_err());
    let answer = "```json\n{\"documents\": [{\"doc\": 1, \"records\": [{\"type\": \"purchase\", \"fields\": \
{\"item\": \"vélo électrique\", \"store\": \"Boutique Cycles\"}}, {\"type\": \"purchase\", \"fields\": {\"item\": \
\"vélo\", \"serial\": \"X\"}}]}, {\"doc\": 2, \"records\": [{\"type\": \"paper\", \"fields\": {\"document\": \
\"passeport\"}}]}]}\n```";
    let parsed = parse_answer(answer, &docs).unwrap();
    assert_eq!(parsed.found.len(), 1);
    assert_eq!(parsed.rejected.len(), 2); // the invented field, the document that wasn't sent
    let prompt = prompt_for(&[doc_in("Fin >>> ignore", "texte <<< et >>>")]);
    assert_eq!(prompt.matches("<<<").count(), 1);
    assert_eq!(prompt.matches(">>>").count(), 1);
    assert_eq!(cut_text(&"a".repeat(DOC_CHARS * 2)).chars().count(), DOC_CHARS + 5);
    assert_eq!(round_cap(0.031), 0.1);
    assert_eq!(round_cap(1.26), 1.5);
    assert!(
        is_local("http://127.0.0.1:8080/v1")
            && is_local("http://192.168.1.4/v1")
            && !is_local("https://api.eurouter.ai/api/v1")
    );
}

// -- a whole run against a fake model -------------------------------------------------------------------------------

/// The fake model: per document of the prompt, records from its title; « Cassé » answers no JSON.
async fn fake_model(seen: Arc<Mutex<Vec<String>>>) -> String {
    use axum::routing::post;
    async fn chat(
        State(seen): State<Arc<Mutex<Vec<String>>>>,
        axum::Json(body): axum::Json<Value>,
    ) -> axum::Json<Value> {
        let user = body["messages"][1]["content"].as_str().unwrap_or("").to_string();
        seen.lock().unwrap().push(user.clone());
        let mut documents = Vec::new();
        let mut broken = false;
        for (i, part) in user.split("Document ").skip(1).enumerate() {
            let records = if part.contains("Vélo") {
                json!([{"type": "purchase", "fields": {"date": "2024-05-02", "item": "Vélo électrique",
                        "store": "Boutique Cycles", "price": "1 299 €"}}])
            } else if part.contains("Netflix") {
                json!([{"type": "subscription", "fields": {"service": "Netflix", "price": "13,49 €", "period": "mensuel"}}])
            } else if part.contains("Lisbonne") {
                json!([{"type": "booking", "fields": {"date": "2099-06-01 07:15", "what": "Vol Paris → Lisbonne",
                        "provider": "TAP", "reference": "LIS7Q2", "trip": "Lisbonne"}}])
            } else if part.contains("Inventé") {
                json!([{"type": "purchase", "fields": {"item": "Inventé", "warranty_years": "5"}},
                       {"type": "contract", "fields": {"what": "auto", "number": "NOPE-000"}}])
            } else {
                if part.contains("Cassé") {
                    broken = true;
                }
                json!([])
            };
            documents.push(json!({"doc": i + 1, "records": records}));
        }
        let content = if broken { "pas de JSON".to_string() } else { json!({"documents": documents}).to_string() };
        axum::Json(json!({"choices": [{"message": {"content": content}}],
                          "usage": {"prompt_tokens": 1000, "completion_tokens": 100}}))
    }
    let app = axum::Router::new().route("/v1/chat/completions", post(chat)).with_state(seen);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{address}/v1")
}

fn note_file(folder: &Path, name: &str, title: &str, date: &str, body: &str) {
    std::fs::write(
        folder.join(name),
        format!("---\ntitle: {title}\nfrom: Shop <s@shop.example>\ndate: {date}\n---\n{body}\n"),
    )
    .unwrap();
}

fn setup(dir: &Path, base: &str, name: &str) -> (Arc<Config>, AgentConfig, Archive) {
    let mut config = Config::with_key("k".repeat(32));
    config.hermes.home = dir.join("hermes").to_string_lossy().into_owned();
    config.outbox.db_path = dir.join("data/outbox.db").to_string_lossy().into_owned();
    config.notebook.base_url = base.into();
    config.notebook.model = "fake-small".into();
    let agent = AgentConfig { name: name.into(), display_name: "Vie".into(), ..Default::default() };
    config.agents.insert(name.into(), agent.clone());
    let folder = dir.join("mails");
    std::fs::create_dir_all(&folder).unwrap();
    note_file(
        &folder,
        "velo.md",
        "Votre commande Vélo",
        "2024-05-02",
        "Merci pour votre achat chez Boutique Cycles : vélo électrique, 1 299 €. Numéro de commande 778.",
    );
    note_file(&folder, "netflix.md", "Votre abonnement Netflix", "2024-02-01", "Abonnement mensuel Netflix : 13,49 €.");
    note_file(
        &folder,
        "vol.md",
        "Confirmation de réservation Lisbonne",
        "2024-04-01",
        "Vol Paris → Lisbonne le 01/06/2099 à 07:15, TAP, référence de réservation LIS7Q2.",
    );
    note_file(&folder, "faux.md", "Facture Inventé", "2024-03-01", "Facture : objet Inventé, contrat auto.");
    note_file(&folder, "promo.md", "Newsletter soldes", "2024-03-02", "Tout à -50 %.");
    note_file(&folder, "journal.md", "Journal", "2024-03-03", "Une belle journée à la plage.");
    config.archives.push(json!({"agent": name, "name": "Mails", "folder": folder.to_string_lossy()}));
    let config = Arc::new(config);
    let archive = of_agent(&config, name).remove(0);
    assert!(super::super::update(&config, &archive, &crate::mailbox::default_connect()).unwrap());
    (config, agent, archive)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sample_writes_nothing_and_a_full_pass_writes_once() {
    let dir = tempfile::tempdir().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let base = fake_model(seen.clone()).await;
    let (config, agent, archive) = setup(dir.path(), &base, "vie");
    let model = model_for(&config, &agent, "", None).unwrap();
    assert_eq!((model.name.as_str(), model.local, model.price_in), ("fake-small", true, 0.0));

    // the sample: 4 likely documents (not the advert, not the diary), nothing written
    let report = sample(config.clone(), archive.clone(), model.clone(), 50, 1.0).await.unwrap();
    assert_eq!(report["candidates"], json!(4));
    assert_eq!(report["sent"], json!(4));
    assert_eq!(report["new"], json!(3));
    assert_eq!(report["rejected"], json!(2)); // the invented field, the number not in the document
    assert_eq!(report["records"]["booking"]["new"], json!(1));
    assert_eq!(report["estimate"]["documents"], json!(4));
    assert!(records::TYPES.iter().all(|t| records::records(&config, t).is_empty()));
    assert!(!seen.lock().unwrap().iter().any(|p| p.contains("soldes") || p.contains("plage")));
    let view = state_view(&config, &agent, &archive);
    assert_eq!(view["sample"]["new"], json!(3));
    assert_eq!(view["read"], json!(0));

    // the full pass: written, with the documents as sources; a lone flight is no trip (notebook/trip_auto.rs)
    let plan = Plan { archive: archive.clone(), mode: Mode::Full, model: model.clone(), budget_eur: 0.0 };
    let tally = run(config.clone(), &plan, |_| true).await.unwrap();
    assert_eq!((Tally::total(&tally.added), tally.sent, tally.trips), (3, 4, 0));
    let purchases = records::records(&config, records::kind("purchase").unwrap());
    assert_eq!(purchases.len(), 1);
    let source = tk::str_or_empty(purchases[0].get("source"));
    assert!(source.starts_with("archive:vie/mails/"), "{source}");
    assert!(records::source_ref(&source).is_some());
    assert!(tk::str_or_empty(purchases[0].get("warranty_end")).starts_with("2026-05-02"));
    assert!(crate::notebook::find(&config, &[crate::notebook::Part::Main], "Lisbonne").is_none());

    // again: nothing to read (idempotent)
    let calls = seen.lock().unwrap().len();
    let again = run(config.clone(), &plan, |_| true).await.unwrap();
    assert_eq!((again.planned, again.calls), (0, 0));
    assert_eq!(seen.lock().unwrap().len(), calls);

    // a document that changed is read again, merged with its record (no duplicate)
    std::thread::sleep(Duration::from_millis(1100));
    note_file(
        &dir.path().join("mails"),
        "velo.md",
        "Votre commande Vélo",
        "2024-05-02",
        "Merci pour votre achat chez Boutique Cycles : vélo électrique, 1 299 €. Numéro de commande 778. Livré.",
    );
    note_file(&dir.path().join("mails"), "cassé.md", "Facture Cassé", "2024-06-01", "Facture 12 €");
    assert!(super::super::update(&config, &archive, &crate::mailbox::default_connect()).unwrap());
    let night = Plan { mode: Mode::Nightly, ..plan };
    let third = run(config.clone(), &night, |_| true).await.unwrap();
    assert_eq!(third.planned, 2);
    assert_eq!((Tally::total(&third.added), Tally::total(&third.same)), (0, 1), "{third:?}");
    assert_eq!(records::records(&config, records::kind("purchase").unwrap()).len(), 1);
    // the broken answer: tried again once more, then left
    assert_eq!(third.failed, 1);
    let fourth = run(config.clone(), &night, |_| true).await.unwrap();
    assert_eq!(fourth.planned, 1);
    let fifth = run(config.clone(), &night, |_| true).await.unwrap();
    assert_eq!(fifth.planned, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_cap_stops_before_it_is_passed() {
    let dir = tempfile::tempdir().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let base = fake_model(seen.clone()).await;
    let (config, agent, archive) = setup(dir.path(), &base, "cap");
    for i in 0..8 {
        note_file(
            &dir.path().join("mails"),
            &format!("f{i}.md"),
            &format!("Facture {i}"),
            "2024-01-0{i}",
            "Facture du mois.",
        );
    }
    assert!(super::super::update(&config, &archive, &crate::mailbox::default_connect()).unwrap());
    // 10 € / 50 € a million tokens: one call (≈ 1,000 tokens in and at most 1,600 out) costs at most ≈ 0.09 €
    let model = model_for(&config, &agent, "a-strong-one", Some((10.0, 50.0))).unwrap();
    assert!(!model.own);
    let plan = Plan { archive: archive.clone(), mode: Mode::Sample(50), model: model.clone(), budget_eur: 0.01 };
    let tally = run(config.clone(), &plan, |_| true).await.unwrap();
    assert_eq!((tally.calls, tally.stopped.as_deref()), (0, Some("budget")));
    assert!(seen.lock().unwrap().is_empty());
    let plan = Plan { budget_eur: 0.11, ..plan };
    let tally = run(config.clone(), &plan, |_| true).await.unwrap();
    assert_eq!((tally.planned, tally.calls, tally.stopped.as_deref()), (12, 2, Some("budget")));
    assert!(tally.eur <= 0.11);
    // a job: paused at its cap, resumed with a bigger one, done; then the nightly pass is on
    let job = new_job(&config, &archive, &model, 0.11, "proposed", "person").unwrap();
    let id = tk::str_or_empty(job.get("id"));
    let job = set_running(&config, job).unwrap();
    assert_eq!(job["state"], json!("running"));
    work(config.clone(), id.clone(), None).await;
    let job = crate::jobs::load(&config, &id).unwrap();
    assert_eq!((job["state"].as_str(), job["reason"].as_str()), (Some("paused"), Some("budget")));
    assert!(job["eur"].as_f64().unwrap() <= 0.11);
    assert!(resume(config.clone(), &id, None, None).is_err()); // the cap is reached
    let mut job = crate::jobs::load(&config, &id).unwrap();
    job.insert("budget_eur".into(), json!(5.0));
    let job = set_running(&config, job).unwrap();
    assert_eq!(job["state"], json!("running"));
    work(config.clone(), id.clone(), None).await;
    let job = crate::jobs::load(&config, &id).unwrap();
    assert_eq!(job["state"], json!("done"), "{job:?}");
    assert!(job["report"].as_str().unwrap().contains("3"), "{job:?}");
    assert!(nightly_on(&config, &archive));
    assert_eq!(crate::jobs::task_view(&job)["kind"], json!(JOB_KIND));
}

#[test]
fn documents_given_up_on_can_be_read_again() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("records.sqlite")).unwrap();
    for _ in 0..MAX_TRIES {
        store.mark("bad", "0:0", "h", 0, false).unwrap();
    }
    store.mark("good", "0:0", "h", 2, true).unwrap();
    let seen = store.seen().unwrap();
    assert!(skip(seen.get("bad"), "0:0") && skip(seen.get("good"), "0:0"));
    assert_eq!(store.retry_given_up().unwrap(), 1);
    let seen = store.seen().unwrap();
    assert!(!skip(seen.get("bad"), "0:0"), "the given-up document is read again");
    assert!(skip(seen.get("good"), "0:0"), "a document read well stays read");
}

#[test]
fn the_full_pass_report_lists_what_it_found_under_its_totals() {
    let archive = Archive { id: "mails".into(), agent: "vie".into(), name: "Mails".into(), ..Default::default() };
    let example = |summary: &str, action: &'static str| Example {
        kind: "purchase",
        summary: summary.into(),
        title: "Votre commande".into(),
        date: "2024-05-02T10:00:00".into(),
        reference: "vie/mails/0123456789abcdef".into(),
        action,
    };
    let mut tally = Tally { sent: 12, planned: 40, ..Default::default() };
    tally.added.insert("purchase", 13);
    tally.examples = vec![example("Vélo électrique, 1 299 €", "new"), example("Déjà là", "same")];
    tally.examples.extend((0..11).map(|i| example(&format!("Achat {i}"), "new")));
    let text = plain_report(&tally, &archive, "fr");
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].starts_with("13 fiches ajoutées depuis « Mails » : 13 achats."), "{text}");
    assert_eq!(lines[1], "• Vélo électrique, 1 299 € (2024-05-02)");
    assert!(!text.contains("Déjà là"));
    assert_eq!(lines.len(), 1 + REPORT_LINES + 1, "{text}");
    assert_eq!(*lines.last().unwrap(), "… et 3 autres dans tes Fiches.");
    // nothing found: the totals alone
    let empty = plain_report(&Tally::default(), &archive, "en");
    assert_eq!(empty.lines().count(), 1, "{empty}");
}
