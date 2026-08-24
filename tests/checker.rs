use dynon_usb_updater::checker::{self, SimpleDate};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

#[test]
fn the_experimental_page_yields_one_current_listing() {
    let html = fixture("dynon-experimental.html");
    let listings = checker::parse_dynon_page(&html);
    // Its "Upcoming Data" table is commented out on the real page — must not
    // be mistaken for a second, live listing.
    assert_eq!(listings.len(), 1, "commented-out block leaked through");

    let listing = &listings[0];
    assert_eq!(
        listing.href,
        "/downloads/Software/Us-av-ob/FAA_av2608_ob2604.duc"
    );
    assert_eq!(listing.filename(), "FAA_av2608_ob2604.duc");
    assert_eq!(listing.aviation_cycle.unwrap().label(), "2608");
    assert_eq!(listing.obstacle_cycle.unwrap().label(), "2604");

    let av = listing.aviation_valid.as_ref().unwrap();
    assert_eq!(av.text, "August 6 - September 2");
    assert_eq!(
        av.start,
        SimpleDate {
            year: 2026,
            month: 8,
            day: 6
        }
    );
    assert_eq!(
        av.end,
        SimpleDate {
            year: 2026,
            month: 9,
            day: 2
        }
    );
}

#[test]
fn the_certified_page_yields_current_and_upcoming_separately() {
    let html = fixture("dynon-certified.html");
    let listings = checker::parse_dynon_page(&html);
    assert_eq!(listings.len(), 2);

    assert_eq!(listings[0].href, "software/data/FAA_av2607_ob2604.duc");
    assert_eq!(listings[0].aviation_cycle.unwrap().label(), "2607");
    assert_eq!(
        listings[0].aviation_valid.as_ref().unwrap().text,
        "July 9 - August 5"
    );

    assert_eq!(listings[1].href, "software/data/FAA_av2608_ob2604.duc");
    assert_eq!(listings[1].aviation_cycle.unwrap().label(), "2608");
    assert_eq!(
        listings[1].aviation_valid.as_ref().unwrap().text,
        "August 6 - September 2"
    );
    // Both entries carry the same obstacle cycle — the two databases are
    // genuinely independent and must never be collapsed into one number.
    assert_eq!(listings[0].obstacle_cycle, listings[1].obstacle_cycle);
    assert_eq!(listings[1].obstacle_cycle.unwrap().label(), "2604");
}

/// The whole point of parsing real validity windows instead of trusting the
/// page's own "Current"/"Upcoming" labels: on the real fetched page (see
/// tests/data/dynon-certified.html), the section literally labelled
/// "Current Data" was already stale — cycle 2607 had expired — while the
/// section labelled "Upcoming Data" (cycle 2608) was the one actually valid
/// that day. `select` must get this right by date, not by label.
#[test]
fn select_picks_by_validity_window_not_by_the_pages_own_label() {
    let html = fixture("dynon-certified.html");
    let listings = checker::parse_dynon_page(&html);

    // A date inside the page-labelled "Current Data" window.
    let mid_july = SimpleDate {
        year: 2026,
        month: 7,
        day: 20,
    };
    match checker::select(&listings, mid_july) {
        Some(checker::Selection::Current(l)) => {
            assert_eq!(l.aviation_cycle.unwrap().label(), "2607")
        }
        other => panic!("expected the labelled-current 2607 listing, got {other:?}"),
    }

    // A date inside the page-labelled "Upcoming Data" window — the real
    // scenario this app was built to get right.
    let late_august = SimpleDate {
        year: 2026,
        month: 8,
        day: 24,
    };
    match checker::select(&listings, late_august) {
        Some(checker::Selection::Current(l)) => {
            assert_eq!(l.aviation_cycle.unwrap().label(), "2608")
        }
        other => panic!("expected the labelled-upcoming 2608 listing to be current, got {other:?}"),
    }

    // A date before either window: nothing covers it, but 2607 (the
    // earliest future one at that point) should be surfaced as upcoming.
    let before_both = SimpleDate {
        year: 2026,
        month: 6,
        day: 1,
    };
    match checker::select(&listings, before_both) {
        Some(checker::Selection::UpcomingOnly(l)) => {
            assert_eq!(l.aviation_cycle.unwrap().label(), "2607")
        }
        other => panic!("expected 2607 surfaced as upcoming, got {other:?}"),
    }
}

#[test]
fn a_single_listing_is_always_current_regardless_of_date() {
    let html = fixture("dynon-experimental.html");
    let listings = checker::parse_dynon_page(&html);
    let far_future = SimpleDate {
        year: 2030,
        month: 1,
        day: 1,
    };
    assert!(matches!(
        checker::select(&listings, far_future),
        Some(checker::Selection::Current(_))
    ));
}

#[test]
fn no_listings_selects_nothing() {
    assert_eq!(
        checker::select(
            &[],
            SimpleDate {
                year: 2026,
                month: 1,
                day: 1
            }
        ),
        None
    );
}

#[test]
fn page_url_is_gated_by_system_type_never_inferred() {
    assert_eq!(checker::page_url("certified"), checker::CERTIFIED_PAGE_URL);
    assert_eq!(
        checker::page_url("experimental"),
        checker::EXPERIMENTAL_PAGE_URL
    );
    // An unset/unknown value must not silently resolve to certified — the
    // safer of the two wrong guesses is the one that behaves like
    // experimental (no STC claim implied).
    assert_eq!(checker::page_url("unset"), checker::EXPERIMENTAL_PAGE_URL);
}

#[test]
fn resolve_url_handles_both_pages_href_styles() {
    // Certified: a relative href.
    assert_eq!(
        checker::resolve_url(
            checker::CERTIFIED_PAGE_URL,
            "software/data/FAA_av2608_ob2604.duc"
        )
        .unwrap(),
        "https://www.dynoncertified.com/software/data/FAA_av2608_ob2604.duc"
    );
    // Experimental: a root-relative href.
    assert_eq!(
        checker::resolve_url(
            checker::EXPERIMENTAL_PAGE_URL,
            "/downloads/Software/Us-av-ob/FAA_av2608_ob2604.duc"
        )
        .unwrap(),
        "https://dynonavionics.com/downloads/Software/Us-av-ob/FAA_av2608_ob2604.duc"
    );
}
