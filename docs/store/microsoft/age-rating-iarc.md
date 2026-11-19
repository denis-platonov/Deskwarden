# Microsoft Store: age rating (IARC questionnaire)

Partner Center -> *Age ratings* runs the IARC questionnaire (the same one
Google Play and others use). These are Deskwarden's honest answers as of 1.0;
**the owner submits it and must re-read each question as worded then.** Pick
the category that fits a utility. The expected result is the lowest rating
(PEGI 3 / ESRB Everyone / USK 0).

| Topic | Answer | Why |
|---|---|---|
| App category | Utility, productivity or tool | A password manager / autofill utility; not a game, not social |
| Violence, blood, fear | No | None in the app |
| Sexual content, nudity | No | None |
| Language / profanity | No | None. The generator makes random strings; passphrases draw from the EFF word list |
| Controlled substances (drugs, alcohol, tobacco) | No | None |
| Gambling or simulated gambling | No | None |
| Users can interact / user-generated content | **No** | No chat, feed, public content or contact with strangers. Bitwarden Send shares a secret by private link with a person the user chooses. If the form insists on a yes for "user-to-user sharing", answer it as private, user-initiated sharing of the user's own data |
| Shares the user's location | No | Not used |
| Shares personal information with third parties | **No** | Nothing is sent to the developer or sold. Traffic goes to the user's own vault server and, optionally, an icon service (a domain name) and a breach service (five characters of a hash); never account data. See `PRIVACY.md` |
| Digital purchases / in-app purchases | No | Free; none |
| Unrestricted internet access | Answer as the form words it | It connects only to the addresses `PRIVACY.md` lists and does not browse content; it is not a browser |
| Advertising | No | None |
| Collects personal information | Answer "no data collected by the developer" | There is no developer-side collection; the app handles the user's own vault on the user's PC |

Contact email for the rating certificate: the support address chosen in
`listing.md`. Keep the generated IARC certificate. A later release that adds a
social, sharing-with-strangers or purchasing feature needs the questionnaire
redone.
