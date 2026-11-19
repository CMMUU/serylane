# Guide d’utilisation de Serylane

Installez le client, importez votre abonnement ou votre fichier YAML, choisissez un mode proxy et utilisez le routage AI, les mises à jour et le diagnostic.

## Télécharger et installer
<!-- anchor: install -->

Récupérez la version stable vérifiée sur le [site officiel](https://serylane.cmmuu.com/fr/#download) ou au [centre de téléchargement](https://downloads.cmmuu.com/projects/serylane). Le client ne fournit ni nœuds, ni abonnements, ni comptes AI.

- Windows : x64 pour un PC Intel ou AMD 64 bits, ARM64 pour Windows sur Arm.
- macOS : choisissez Puce Apple pour la série M ou Intel pour un Mac Intel. Ouvrez le DMG et glissez Serylane dans Applications. macOS 13 ou ultérieur est requis.
- Linux : choisissez l’AppImage adaptée, rendez-la exécutable et lancez-la.

## Premiers pas
<!-- anchor: quick-start -->

1. Ajoutez le nom et l’URL d’un abonnement Mihomo ou Clash Meta. Pour un YAML local, ouvrez-le dans Profils et vérifiez-le avant de l’activer.
2. Confirmez que le bon profil est sélectionné. Sélectionner un profil ne démarre pas le proxy.
3. Activez le proxy système dans la barre supérieure, ouvrez un site connu et consultez Connexions pour vérifier l’entrée dans le moteur et la règle appliquée.

## Choisir un mode proxy
<!-- anchor: proxy-modes -->

- **Proxy système** : pour les applications qui lisent les paramètres du système. Sous Linux, il utilise gsettings de GNOME.
- **TUN** : pour les applications qui ignorent le proxy système. Sous macOS, utilisez le programme d’installation complet, signé et notarié, et autorisez le service auxiliaire dans les réglages des éléments d’ouverture si demandé. Sous Windows, TUN est expérimental et requiert des droits administrateur ; il n’est pas implémenté sur Linux.
- **Port local** : renseignez `127.0.0.1` et le port mixte dans l’application cliente, en HTTP ou SOCKS5. N’utilisez pas le port de contrôle.

Un auxiliaire prêt ne prouve pas que le trafic entre dans le tunnel. Vérifiez le mode et une connexion réelle.

## Gérer les abonnements
<!-- anchor: subscriptions -->

Actualisez une carte ou tous les abonnements. Les quotas et la configuration sont actualisés séparément ; en cas d’échec, la dernière configuration utilisable est conservée. Modifiez une carte par le menu Plus ou le clic droit, puis actualisez après un changement d’URL. Si l’import échoue, vérifiez le lien Mihomo ou Clash Meta et le chemin de téléchargement utilisé.

## Sélection manuelle et proxy AI
<!-- anchor: nodes -->

Les modes de sélection sont mutuellement exclusifs. Choisissez un nœud dans un groupe et mesurez sa latence, ou utilisez le proxy AI avec 1 à 10 nœuds admissibles. Avec un seul nœud, il n’y a pas de secours. Appliquez votre sélection et attendez sa confirmation, ou réactivez la sélection automatique. Le changement concerne les connexions suivantes.

## Outils AI, routage et Codex
<!-- anchor: ai-tools -->

Le routage par domaine est désactivé par défaut et ne concerne que les requêtes qui entrent dans le moteur en mode Règles. Sélectionnez outils ou domaines et une sortie, enregistrez le brouillon, puis activez ou appliquez. `example.com` désigne le domaine racine ; `*.example.com` ne couvre que les sous-domaines. [Routage par domaine](https://serylane.cmmuu.com/fr/docs/routing).

Sur macOS ou Windows, lancez une version compatible de la CLI Codex avec `codex --enable respect_system_proxy` pour lire le proxy système existant lors de ce lancement. Cela n’active pas PAC et n’ajoute aucun basculement. Consultez la [documentation OpenAI](https://learn.chatgpt.com/docs/enterprise/managed-configuration#use-the-system-proxy-for-native-client-requests) et validez avec une requête réelle terminée.

## Vérifier et installer les mises à jour
<!-- anchor: updates -->

Dans Réglages → Mise à jour, vérifiez la version et l’origine. Le client essaie GitHub direct, l’accélérateur GitHub du projet, GitHub via le proxy Serylane actif puis le centre de téléchargement. Le paquet doit correspondre à la version et à l’architecture, et sa signature est vérifiée. Si la mise à jour interne échoue, remplacez l’application par l’installateur officiel correspondant sans effacer la configuration. [Vérification](https://serylane.cmmuu.com/fr/docs/verification).

## Dépanner
<!-- anchor: troubleshooting -->

- Le trafic ne passe pas : vérifiez que le proxy système ou TUN est actif, puis regardez Connexions.
- TUN sur macOS attend l’autorisation : vérifiez les éléments d’ouverture. Arrêtez le proxy avant une réassociation du service.
- Port déjà utilisé : ouvrez le détail technique et, après l’arrêt de TUN, attendez la libération du port.
- HTTP 401 : l’interface est joignable, mais l’identification et une réponse réelle restent à confirmer.

Dans un rapport, indiquez version, système, architecture, mode, heure et erreur expurgée, sans URL d’abonnement, jeton ou clé.
