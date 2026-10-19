//! Argos — bot de Telegram: agenda (Google), tarefas que cobram, memória, manutenção e abastecimento por
//! foto. Biblioteca: o binário (`main.rs`) é só a CLI, e um complemento pode usar os módulos (Telegram,
//! token do Google, config, util) sem copiar código.
pub mod agenda;
pub mod atalhos;
pub mod bot;
pub mod clima;
pub mod conversa;
pub mod desfazer;
pub mod gcal;
pub mod instalar;
pub mod motoregis;
pub mod postos;
pub mod rascunho;
pub mod recados;
pub mod rede;
pub mod rotina;
pub mod tarefas;
pub mod temporal;
pub mod util;
#[cfg(test)]
mod testes;
