# Guide du labo : Azimut en 5 minutes

Ce guide suffit pour traiter un vol DJI. Le reste de la documentation est dans le
[README](../README.fr.md) et le dossier [docs](.).

## 1. Installer (une seule fois)

**Windows** : ouvrir *PowerShell* et coller :

```powershell
irm https://raw.githubusercontent.com/tofunori/azimut/main/scripts/install.ps1 | iex
```

**macOS ou Linux** : ouvrir le *Terminal* et coller :

```sh
curl -fsSL https://raw.githubusercontent.com/tofunori/azimut/main/scripts/install.sh | sh
```

C'est tout : RTKLIB-EX, qui fait le calcul GNSS, est inclus. Ouvrir un nouveau terminal et
vérifier avec `azimut --version`.

> Sans terminal, on peut aussi télécharger l'archive de son système dans les
> [Releases](https://github.com/tofunori/azimut/releases), la décompresser et utiliser
> `azimut` depuis ce dossier.
> Windows peut afficher « Windows a protégé votre ordinateur » : *Informations
> complémentaires* → *Exécuter quand même* (le programme n'est pas signé).

## 2. Traiter un vol

Il faut :
- la copie de la carte SD du drone (dossier `DJI_…` avec les images, le `.MRK` et le
  `PPKRAW.bin`) ;
- le fichier RINEX de la station de base qui enregistrait pendant le vol ;
- les coordonnées de la base : idéalement le rapport `.sum` de
  [CSRS-PPP](https://webapp.csrs-scrs.nrcan-rncan.gc.ca/geod/tools-outils/ppp.php) (RNCan) pour
  ce même RINEX.

Azimut peut aussi soumettre lui-même le RINEX de la base à CSRS-PPP et attendre le résultat
(environ une minute) : donner le courriel du compte CSRS-PPP une fois pour toutes dans la
variable `AZIMUT_CSRS_EMAIL`, puis remplacer `--base-ppp …` par `--submit-ppp` à l'étape 2.
Le rapport de CSRS-PPP (`.sum`, `.pdf`…) est gardé dans `D:\resultats\csrs_ppp`. Pour
seulement obtenir la position de la base : `azimut ppp D:\base\base.24o --out D:\base\ppp`.

```sh
# 1. Diagnostic : rien n'est modifié, quelques secondes
azimut scan D:\terrain\2026-08-09 --base D:\base\base.24o

# 2. Calcul PPK, rapport en français, hauteurs CGVD2013
azimut process D:\terrain\2026-08-09 --base D:\base\base.24o ^
    --base-ppp D:\base\12345678.sum --geoid ca_nrc_CGG2013an83.tif ^
    --report-lang fr --out D:\resultats
```

(Sur macOS et Linux, remplacer `^` par `\` en fin de ligne.) La grille du géoïde
`ca_nrc_CGG2013an83.tif` se télécharge sur [cdn.proj.org](https://cdn.proj.org/ca_nrc_CGG2013an83.tif).

## 3. Lire le résultat

Ouvrir `D:\resultats\<vol>_report.html` dans un navigateur :

| Regarder | Bon signe |
|---|---|
| Le bandeau en haut | Vert « Rien à signaler », ou des points à vérifier qu'on comprend |
| Captures fixées | Toutes ou presque (barre verte) |
| Incertitude | Quelques centimètres ou moins |
| Base | Distance de quelques km, enregistrement avant et après le vol |

[Voir un exemple de rapport](../examples/demo-flight/rapport-fr.png).

## 4. Utiliser les positions

- **PIX4Dmatic** : suivre `<vol>_pix4dmatic.txt` (importer `<vol>_pix4dmatic.csv` après les
  images).
- **Metashape** : importer `<vol>_positions.csv`.
- **OpenDroneMap** : `<vol>_geo.txt`.
- **Écrire les positions dans des copies des images** (les originaux ne sont pas touchés) :

  ```sh
  azimut write-images D:\resultats\<vol>_solution.json --mode copy
  ```

## 5. Avec Claude ou Codex (optionnel)

Dans le dossier du projet :

```sh
claude mcp add azimut -- azimut mcp
```

Puis demander simplement : « Fais le PPK du vol dans `terrain/2026-08-09` avec la base dans
`base/`, coordonnées du rapport CSRS-PPP, rapport en français. » L'agent pose les questions
nécessaires (base, cadre de référence, écriture des images) au lieu de deviner.

## En cas de problème

- **« RTKLIB-EX programs not found »** : utiliser l'archive des Releases (RTKLIB inclus) ou
  ajouter `--rtklib <dossier>`.
- **Une capture sans position** : voir `<vol>_problems.csv` et le rapport.
- **Autre chose** : ouvrir une [issue](https://github.com/tofunori/azimut/issues) avec la
  sortie de `azimut scan`.
