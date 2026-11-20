<!-- docs -->
# Démarrage et systèmes pris en charge

Installez la version officielle correspondant à votre architecture et utilisez votre abonnement ou vos réglages Mihomo. Découvrez les fonctions, limites et archives.

## Premiers pas {#start}

Ajoutez puis sélectionnez un abonnement, ou importez un YAML depuis Profils. Activez le proxy système et vérifiez une connexion. Suivez les étapes du [guide d’utilisation](site:guide).

## Fonctions actuelles {#current-capabilities}

Gérez les abonnements et les nœuds et consultez connexions, journaux et diagnostics. Le proxy AI et la sélection manuelle s’excluent. Le [routage par domaine](site:routing) est désactivé par défaut ; les nouvelles intégrations Codex et le proxy par application sont désactivés.

## TUN et limites des systèmes {#tun-compatibility}

macOS exige un installateur complet signé avec Developer ID, notarié par Apple et l’autorisation de l’auxiliaire. TUN sur Windows est expérimental et requiert des droits administrateur. Il n’est pas implémenté sur Linux ; le proxy système utilise gsettings de GNOME.

## Télécharger la bonne architecture {#download-center}

Sur la [page de téléchargement](site:home#download), choisissez x64 ou ARM64 selon votre système. Pour un Mac de la série M, choisissez Puce Apple. Consultez le [centre de téléchargement](https://downloads.cmmuu.com/projects/serylane) pour les versions stables.

## Historique {#history}

Les notes anciennes restent dans les [archives anglaises](/docs/#history) ou [chinoises (simplifiées)](/zh/docs/#history). Elles n’indiquent pas quelle version est disponible aujourd’hui.

<!-- routing -->
# Choisir une sortie selon le domaine

Envoyez les connexions correspondantes qui entrent déjà dans le moteur vers un groupe choisi. Le routage par domaine est désactivé par défaut.

## Outils et domaines personnalisés {#node-metadata}

Utilisez les préréglages OpenAI, Claude et Gemini ou vos propres domaines. `example.com` couvre la racine ; `*.example.com` ne couvre que les sous-domaines. Désélectionner un outil ne supprime pas vos entrées.

## Enregistrer puis appliquer {#save-apply}

Choisissez une sortie de l’abonnement actif, enregistrez le brouillon puis confirmez l’activation ou l’application. Les connexions hors liste gardent leurs règles. Si la sortie a disparu, choisissez-en une autre ; aucune connexion directe n’est substituée.

## Commandes communes {#global-controls}

Le bouton général et la page de routage partagent le même état. L’actualisation de l’état ne démarre pas le proxy et ne bascule pas entre proxy système et TUN.

## Vérifier l’application {#enablement}

Vérifiez le mode Règles, les règles chargées et l’entrée du trafic. Un brouillon enregistré ne prouve rien ; consultez une nouvelle connexion et sa sortie.

## Quand le proxy s’arrête {#proxy-routing-state}

L’arrêt du moteur suspend le routage sans effacer les réglages. La vérification reprend quand vous démarrez le proxy. Le routage seul ne le lance pas.

## Nœuds et stabilité {#stability}

Le proxy AI prend 1 à 10 nœuds admissibles et s’exclut avec la sélection manuelle. Un nœud seul n’a pas de secours. Le changement concerne les prochaines connexions.

## Limites {#validation-boundaries}

Pas de déchiffrement TLS, de routage par chemin chiffré ni de modification de Codex. Nouvelles intégrations Codex, proxy par application et ancienne stratégie de stabilité avancée sont désactivés. [Outils AI](site:guide#ai-tools).

<!-- verification -->
# Vérifier les téléchargements et les mises à jour

Confirmez la provenance, le nom exact du fichier, l’architecture, le SHA-256 et la signature. Les autorisations du système sont indépendantes.

## Provenance {#download-origin}

Choisissez un programme d’installation sur le [site officiel](site:home#download) ou dans le [centre de téléchargement](https://downloads.cmmuu.com/projects/serylane). Les liens vers le code sur GitHub ne servent pas de secours pour les téléchargements.

## Comparer le SHA-256 {#checksums}

Comparez le nom précis et le condensat avec `SHA256SUMS.txt` de la même version. N’installez pas si la valeur diffère. Remplacez les exemples par le nom réel.

- Windows : `Get-FileHash -LiteralPath ./installer.exe -Algorithm SHA256`
- macOS : `shasum -a 256 ./installer.dmg`
- Linux : `sha256sum ./installer.AppImage`

## Signature de mise à jour {#update-signatures}

La mise à jour vérifie la signature du projet. Pour une vérification manuelle, utilisez la signature, le manifeste et la clé publique de la version correspondante.

## Signature du système {#system-signing}

La signature du projet ne remplace ni Windows Authenticode, ni Apple Developer ID, ni la notarisation Apple. Vérifiez les informations de la version.

## TUN sous macOS {#macos-tun}

Utilisez un installateur complet signé et notarié. L’erreur `EX_CONFIG` d’un ancien auxiliaire nécessite la suppression de l’ancien enregistrement et la vérification du nouveau. Autorisez-le dans les éléments d’ouverture si demandé, puis vérifiez un trafic réel.

## Téléchargement interrompu {#update-downloads}

La mise à jour essaie GitHub direct, l’accélérateur, GitHub via le proxy Serylane actif puis le centre de téléchargement. Tout autre trajet conserve version, architecture, taille, SHA-256 et signature.

## Échec de l’installation {#update-installation}

Vérifiez l’étape, l’espace disponible et les permissions. Si l’ancien mécanisme échoue, utilisez l’installateur complet correspondant au même emplacement et gardez la configuration.

## Vérifications d’anciennes versions {#verification-history}

Voir les [archives anglaises](/docs/verification#verification-history) ou [chinoises (simplifiées)](/zh/docs/verification#verification-history).

<!-- privacy -->
# Confidentialité et données traitées

Le site, le centre de téléchargement et le client de bureau sont indépendants. Consulter ce site ne lance pas le proxy et ne relie pas Codex.

## Activité du site {#website-data}

Le site sert du contenu statique et des informations publiques de version. Il n’a ni comptes, ni outils d’analyse, ni traceurs publicitaires. Il ne sollicite pas la configuration et ne contacte pas votre proxy local.

## Préférence locale {#theme-preference}

Seul le thème `serylane-theme` est stocké dans le navigateur et il n’est pas envoyé au serveur. La langue est déterminée par l’URL, sans préférence enregistrée ni cookie de suivi.

## Hébergement {#hosting}

L’hébergeur traite des données ordinaires comme IP, heure, chemin et navigateur pour la livraison et la sécurité.

## Téléchargements et liens externes {#downloads}

La vérification utilise les métadonnées publiques et le fichier correspondant, sans transférer les cookies ni l’autorisation du visiteur. Les services externes traitent leurs requêtes séparément. [Vérification](site:verification).

## Client de bureau {#desktop-client}

Le client traite les abonnements et les règles localement. Le routage est désactivé par défaut et ne déchiffre pas TLS. Les URL d’abonnement peuvent contenir des identifiants à masquer.

## Signaler un problème {#reporting}

Utilisez les [tickets GitHub](https://github.com/CMMUU/serylane/issues) sans publier mots de passe, jetons, URL d’abonnement ou configurations brutes. Vérifiez d’abord le canal privé pour une vulnérabilité.
