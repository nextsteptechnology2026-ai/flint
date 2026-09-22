// Una gramática mínima para probar que un plugin puede traer un lenguaje
// propio. Asignaciones `clave = valor` y comentarios con `#`.
module.exports = grammar({
  name: 'prueba',
  extras: $ => [/\s/],
  rules: {
    documento: $ => repeat(choice($.comentario, $.asignacion)),
    comentario: $ => /#[^\n]*/,
    asignacion: $ => seq($.clave, '=', $.valor),
    clave: $ => /[a-z_]+/,
    valor: $ => choice($.numero, $.cadena),
    numero: $ => /\d+/,
    cadena: $ => /"[^"]*"/,
  },
});
