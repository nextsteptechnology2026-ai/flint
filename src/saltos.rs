//! Lista de saltos: volver a donde se estaba antes de un salto (Alt+←) y
//! rehacer el camino (Alt+→), como atrás y adelante en un navegador.
//!
//! Solo cuentan los saltos, no cada movimiento del cursor: ir a la
//! definición, elegir una coincidencia de la búsqueda en el proyecto, ir a
//! una línea, saltar al siguiente diagnóstico. Si cada flecha dejara una
//! marca, volver sería retroceder de a una letra.
//!
//! Los lugares se guardan por ruta y no por índice de buffer: los índices se
//! corren al cerrar una pestaña, y un archivo cerrado se puede volver a abrir.

use std::path::PathBuf;

/// Cuántos lugares se recuerdan hacia atrás. Más que esto nadie los recorre
/// a mano, y así la lista no crece sin límite en una sesión larga.
const MAX_SALTOS: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lugar {
    pub ruta: PathBuf,
    pub linea: usize,
    pub col: usize,
}

#[derive(Default)]
pub struct ListaSaltos {
    atras: Vec<Lugar>,
    adelante: Vec<Lugar>,
}

impl ListaSaltos {
    /// Anota `desde` como el lugar al que se vuelve con el próximo "atrás".
    /// Un salto nuevo borra el camino hacia adelante, igual que un navegador
    /// cuando se sigue un link después de volver.
    pub fn registrar(&mut self, desde: Lugar) {
        self.adelante.clear();
        // Dos saltos seguidos desde la misma línea (buscar, elegir otra
        // coincidencia) dejarían dos marcas iguales, y volver parecería no
        // hacer nada la primera vez.
        if self.atras.last().is_some_and(|l| misma_linea(l, &desde)) {
            return;
        }
        self.atras.push(desde);
        if self.atras.len() > MAX_SALTOS {
            self.atras.remove(0);
        }
    }

    /// El lugar al que volver, dejando `actual` para poder avanzar después.
    /// Se saltean las marcas que ya son la línea donde está el cursor: volver
    /// a donde uno ya está es un "atrás" que no hace nada. `actual` es `None`
    /// en un buffer sin nombre, que no se puede anotar pero sí dejar.
    pub fn volver(&mut self, actual: Option<Lugar>) -> Option<Lugar> {
        mover(&mut self.atras, &mut self.adelante, actual)
    }

    /// Lo inverso de `volver`.
    pub fn avanzar(&mut self, actual: Option<Lugar>) -> Option<Lugar> {
        mover(&mut self.adelante, &mut self.atras, actual)
    }

    pub fn cuantos_atras(&self) -> usize {
        self.atras.len()
    }

    pub fn cuantos_adelante(&self) -> usize {
        self.adelante.len()
    }
}

/// Saca el próximo destino de `desde` y deja `actual` en `hacia`, que es la
/// pila por la que se deshace el paso.
fn mover(desde: &mut Vec<Lugar>, hacia: &mut Vec<Lugar>, actual: Option<Lugar>) -> Option<Lugar> {
    while let Some(destino) = desde.pop() {
        if actual.as_ref().is_some_and(|a| misma_linea(&destino, a)) {
            continue;
        }
        hacia.extend(actual);
        return Some(destino);
    }
    None
}

/// La columna no cuenta para decidir si dos lugares son el mismo: moverse
/// dentro de la línea después de un salto no es haberse ido a otro lado.
fn misma_linea(a: &Lugar, b: &Lugar) -> bool {
    a.ruta == b.ruta && a.linea == b.linea
}

#[cfg(test)]
mod tests {
    use super::*;

    fn en(ruta: &str, linea: usize) -> Lugar {
        Lugar { ruta: PathBuf::from(ruta), linea, col: 0 }
    }

    #[test]
    fn volver_y_avanzar_recorren_el_camino() {
        let mut s = ListaSaltos::default();
        // a:1 → b:10 → c:20
        s.registrar(en("a", 1));
        s.registrar(en("b", 10));
        assert_eq!(s.volver(Some(en("c", 20))), Some(en("b", 10)));
        assert_eq!(s.volver(Some(en("b", 10))), Some(en("a", 1)));
        assert_eq!(s.volver(Some(en("a", 1))), None);
        assert_eq!(s.avanzar(Some(en("a", 1))), Some(en("b", 10)));
        assert_eq!(s.avanzar(Some(en("b", 10))), Some(en("c", 20)));
        assert_eq!(s.avanzar(Some(en("c", 20))), None);
    }

    #[test]
    fn un_salto_nuevo_borra_el_camino_hacia_adelante() {
        let mut s = ListaSaltos::default();
        s.registrar(en("a", 1));
        assert_eq!(s.volver(Some(en("b", 2))), Some(en("a", 1)));
        assert_eq!(s.cuantos_adelante(), 1);
        s.registrar(en("a", 1));
        assert_eq!(s.cuantos_adelante(), 0);
    }

    #[test]
    fn no_repite_la_misma_linea_seguida() {
        let mut s = ListaSaltos::default();
        s.registrar(en("a", 1));
        s.registrar(Lugar { col: 7, ..en("a", 1) });
        assert_eq!(s.cuantos_atras(), 1);
    }

    #[test]
    fn volver_saltea_la_linea_donde_ya_se_esta() {
        let mut s = ListaSaltos::default();
        s.registrar(en("a", 1));
        s.registrar(en("b", 5));
        // Se volvió a b:5 a mano: el primer "atrás" tiene que llevar a a:1.
        assert_eq!(s.volver(Some(en("b", 5))), Some(en("a", 1)));
    }

    #[test]
    fn tiene_un_tope() {
        let mut s = ListaSaltos::default();
        for i in 0..MAX_SALTOS + 20 {
            s.registrar(en("a", i));
        }
        assert_eq!(s.cuantos_atras(), MAX_SALTOS);
        // Se descartan los más viejos, no los más nuevos.
        assert_eq!(s.volver(Some(en("z", 0))), Some(en("a", MAX_SALTOS + 19)));
    }

    #[test]
    fn desde_un_buffer_sin_nombre_se_vuelve_pero_no_se_anota() {
        let mut s = ListaSaltos::default();
        s.registrar(en("a", 1));
        assert_eq!(s.volver(None), Some(en("a", 1)));
        assert_eq!(s.cuantos_adelante(), 0);
    }
}
