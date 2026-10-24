//! Замер: сколько высоты раскладка Slint выделяет тексту с переносом в разных сочетаниях
//! элементов (`cargo run -p lmq-desktop --example wrap_probe`). Им найден баг Slint 1.18, от
//! которого защищает tests/slint_layout.rs: внутри `if`/`for` текст с переносом получает высоту
//! одной строки (D, F, G, H, M…, O, P, RA — меньше, чем E, I, RC). После обновления Slint
//! запустите снова: если все варианты сравнялись с E, правило теста можно ослабить.

slint::slint! {
    import { ScrollView, LineEdit } from "std-widgets.slint";

    component Row inherits HorizontalLayout {
        in property <string> label;
        in property <string> hint;
        out property <length> hint-h: h.height;
        out property <length> hint-pref: h.preferred-height;
        min-height: 56px;
        padding: 8px;
        spacing: 16px;
        VerticalLayout {
            horizontal-stretch: 1;
            alignment: center;
            Text { text: label; wrap: word-wrap; }
            h := Text { text: hint; wrap: word-wrap; font-size: 12px; }
        }
        HorizontalLayout {
            alignment: end;
            LineEdit { width: 300px; }
        }
    }

    component CondRow inherits HorizontalLayout {
        in property <string> label;
        in property <string> hint;
        min-height: 56px;
        padding: 8px;
        spacing: 16px;
        VerticalLayout {
            horizontal-stretch: 1;
            alignment: center;
            Text { text: label; wrap: word-wrap; }
            if hint != "": Text { text: hint; wrap: word-wrap; font-size: 12px; }
        }
        HorizontalLayout {
            alignment: end;
            LineEdit { width: 300px; }
        }
    }

    component Note inherits HorizontalLayout {
        in property <string> text;
        padding: 16px;
        Text { text: root.text; wrap: word-wrap; }
    }

    // Точная копия SettingRow из settings.slint.
    component SRow inherits HorizontalLayout {
        in property <string> label;
        in property <string> hint;
        in property <bool> locked;
        in property <bool> stack;
        min-height: stack ? 0 : 56px;
        padding-left: 16px;
        padding-right: 16px;
        padding-top: stack ? 10px : 8px;
        padding-bottom: stack ? 6px : 8px;
        spacing: 16px;
        VerticalLayout {
            horizontal-stretch: 1;
            alignment: center;
            spacing: 2px;
            Text { text: label; font-size: 14px; wrap: word-wrap; }
            if hint != "" || locked: Text {
                text: locked ? "locked" : hint;
                font-size: 12px;
                wrap: word-wrap;
            }
        }
        HorizontalLayout {
            alignment: end;
            @children
        }
    }

    // M1: SRow с постоянным min-height.
    component SRow1 inherits HorizontalLayout {
        in property <string> hint;
        min-height: 56px;
        padding: 8px;
        spacing: 16px;
        VerticalLayout {
            horizontal-stretch: 1;
            alignment: center;
            Text { text: "x"; wrap: word-wrap; }
            if hint != "": Text { text: hint; font-size: 12px; wrap: word-wrap; }
        }
        HorizontalLayout { alignment: end; @children }
    }
    // M2: без условного min-height и без @children-обёртки, подсказка без if.
    component SRow2 inherits HorizontalLayout {
        in property <bool> stack;
        in property <string> hint;
        min-height: stack ? 0 : 56px;
        padding: 8px;
        spacing: 16px;
        VerticalLayout {
            horizontal-stretch: 1;
            alignment: center;
            Text { text: "x"; wrap: word-wrap; }
            Text { text: hint; font-size: 12px; wrap: word-wrap; }
        }
        HorizontalLayout { alignment: end; @children }
    }

    component FEd inherits VerticalLayout {
        in property <string> kind;
        in property <string> hint;
        if kind == "text": SRow {
            label: "Интерфейс без туннеля";
            hint: hint;
            VerticalLayout {
                alignment: center;
                if kind == "text": LineEdit { width: 300px; }
            }
        }
    }

    // Условная подсказка, дети без if.
    component FEd1 inherits VerticalLayout {
        in property <string> hint;
        if hint != "": SRow1 { hint: hint; VerticalLayout { alignment: center; LineEdit { width: 300px; } } }
    }
    // Подсказка без if, дети с if.
    component FEd2 inherits VerticalLayout {
        in property <string> hint;
        if hint != "": SRow2 { hint: hint; VerticalLayout { alignment: center; if hint != "": LineEdit { width: 300px; } } }
    }
    // Подсказка без if, дети без if.
    component FEd3 inherits VerticalLayout {
        in property <string> hint;
        if hint != "": SRow2 { hint: hint; VerticalLayout { alignment: center; LineEdit { width: 300px; } } }
    }

    // Ребёнок — LineEdit напрямую, не раскладка.
    component FEd4 inherits VerticalLayout {
        in property <string> hint;
        if hint != "": SRow2 { hint: hint; LineEdit { width: 300px; } }
    }
    // Без if внутри: компонент в for, строка с детьми.
    component FEd5 inherits VerticalLayout {
        in property <string> hint;
        SRow2 { hint: hint; VerticalLayout { alignment: center; LineEdit { width: 300px; } } }
    }

    component Editor inherits VerticalLayout {
        in property <string> hint;
        if hint != "": Row { label: "x"; hint: hint; }
    }

    // Кандидат в общий абзац: раскладка-компонент с текстом, отступы задаются снаружи.
    component Para inherits HorizontalLayout {
        in property <string> text;
        in property <length> size: 14px;
        Text { text: root.text; font-size: root.size; wrap: word-wrap; }
    }
    // Только отступ снаружи, без свойства шрифта.
    component ParaA inherits HorizontalLayout {
        in property <string> text;
        Text { text: root.text; wrap: word-wrap; }
    }
    // Свойство шрифта, отступ внутри.
    component ParaB inherits HorizontalLayout {
        in property <string> text;
        in property <length> size: 14px;
        padding: 16px;
        Text { text: root.text; font-size: root.size; wrap: word-wrap; }
    }
    // Отступ — свойством компонента, внутри.
    component ParaC inherits HorizontalLayout {
        in property <string> text;
        in property <length> pad: 16px;
        padding: pad;
        Text { text: root.text; wrap: word-wrap; }
    }
    // Как Banner: корень — Rectangle, внутри раскладка с текстом.
    component RectNote inherits Rectangle {
        in property <string> text;
        HorizontalLayout {
            padding: 12px;
            VerticalLayout { Text { text: root.text; wrap: word-wrap; } }
        }
    }
    component Wrapper inherits VerticalLayout {
        in property <bool> show;
        if show: Para { padding: 16px; text: "Через него определяется внешний IP для ссылок — адрес, по которому прокси виден снаружи. Пусто — физический интерфейс с основным шлюзом, а не туннель VPN."; }
    }

    export component Probe inherits Window {
        width: 700px;
        height: 400px;
        property <string> long: "Через него определяется внешний IP для ссылок — адрес, по которому прокси виден снаружи. Пусто — физический интерфейс с основным шлюзом, а не туннель VPN.";
        // A: текст прямо в раскладке.
        out property <length> a-h: a.height;
        out property <length> a-pref: a.preferred-height;
        // B: строка-компонент с подсказкой (без if).
        out property <length> b-row: b.height;
        out property <length> b-hint: b.hint-h;
        out property <length> b-hint-pref: b.hint-pref;
        // C: строка-компонент с подсказкой в if.
        out property <length> c-row: c.height;
        // D: Text в if прямо в раскладке.
        out property <length> d-box: d.height;
        // E: то же без if.
        out property <length> e-box: e.height;
        // F: for + HorizontalLayout { Text }.
        out property <length> f-box: f.height;
        // G: if + VerticalLayout { Text } без отступов.
        out property <length> g-box: g.height;
        // H: if + HorizontalLayout без отступов.
        out property <length> h-box: hh.height;
        // I: if + Text с отступами через обёртку-компонент.
        out property <length> i-box: i.height;
        // J: if + VerticalLayout { компонент с переносом }.
        out property <length> j-box: j.height;
        // K: for + VerticalLayout { строка-компонент }.
        out property <length> k-box: k.height;
        // L: for → компонент → if → строка-компонент (как поля настроек).
        out property <length> l-box: l.height;
        // M: точная форма полей настроек: for → FieldEditor → if → SettingRow с @children.
        out property <length> m-box: m.height;
        out property <length> m1: m1.height;
        out property <length> m2: m2.height;
        out property <length> m3: m3.height;
        out property <length> m4: m4.height;
        out property <length> m5: m5.height;
        out property <length> m6: m6.height;
        out property <length> m7: m7.height;
        out property <length> o: o.height;
        out property <length> pp: pp.height;
        out property <length> q1: q1.height;
        out property <length> q2: q2.height;
        out property <length> q3: q3.height;
        out property <length> q4: q4.height;
        out property <length> ra: ra.height;
        out property <length> rb: rb.height;
        out property <length> rc: rc.height;
        out property <length> rd: rd.height;
        out property <length> re: re.height;
        out property <length> s1: s1.height;
        out property <length> s2: s2.height;
        out property <length> s3: s3.height;
        out property <length> n0: n0.height;
        out property <length> n1: n1.height;
        out property <length> n2: n2.height;
        out property <length> n3: n3.height;
        out property <length> n4: n4.height;
        out property <length> content-pref: content.preferred-height;
        out property <length> content-h: content.height;
        sv := ScrollView {
            content-width: self.visible-width;
            content-height: content.preferred-height;
            content := VerticalLayout {
                width: sv.visible-width;
                padding: 20px;
                spacing: 10px;
                a := Text { text: long; wrap: word-wrap; }
                b := Row { label: "Интерфейс без туннеля"; hint: long; }
                c := CondRow { label: "Интерфейс без туннеля"; hint: long; }
                d := VerticalLayout {
                    if true: HorizontalLayout { padding: 16px; Text { text: long; wrap: word-wrap; } }
                }
                e := VerticalLayout {
                    HorizontalLayout { padding: 16px; Text { text: long; wrap: word-wrap; } }
                }
                f := VerticalLayout {
                    for x in [long]: HorizontalLayout { padding: 16px; Text { text: x; wrap: word-wrap; } }
                }
                g := VerticalLayout {
                    if true: VerticalLayout { Text { text: long; wrap: word-wrap; } }
                }
                hh := VerticalLayout {
                    if true: HorizontalLayout { Text { text: long; wrap: word-wrap; } }
                }
                i := VerticalLayout {
                    if true: Note { text: long; }
                }
                j := VerticalLayout {
                    if true: VerticalLayout { Note { text: long; } }
                }
                k := VerticalLayout {
                    for x in [long]: VerticalLayout { Row { label: "x"; hint: x; } }
                }
                l := VerticalLayout {
                    for x in [long]: Editor { hint: x; }
                }
                m := VerticalLayout {
                    for x in [long]: FEd { kind: "text"; hint: x; }
                }
                // Прямо в странице, без for/if — чтобы отделить сам компонент от повторителя.
                m1 := VerticalLayout { for x in [long]: FEd1 { hint: x; } }
                m2 := VerticalLayout { for x in [long]: FEd2 { hint: x; } }
                m3 := VerticalLayout { for x in [long]: FEd3 { hint: x; } }
                m4 := VerticalLayout { for x in [long]: FEd4 { hint: x; } }
                m5 := VerticalLayout { for x in [long]: FEd5 { hint: x; } }
                m6 := VerticalLayout { if true: SRow2 { hint: long; VerticalLayout { alignment: center; LineEdit { width: 300px; } } } }
                m7 := VerticalLayout { for x in [long]: SRow2 { hint: x; LineEdit { width: 300px; } } }
                o := VerticalLayout { if true: VerticalLayout { SRow2 { hint: long; VerticalLayout { alignment: center; LineEdit { width: 300px; } } } } }
                pp := VerticalLayout { for x in [long]: VerticalLayout { SRow2 { hint: x; LineEdit { width: 300px; } } } }
                q1 := VerticalLayout { if true: Para { padding: 16px; text: long; } }
                q2 := VerticalLayout { for x in [long]: Para { padding: 16px; text: x; } }
                q3 := VerticalLayout { if true: Wrapper { show: true; } }
                q4 := VerticalLayout { if true: VerticalLayout { if true: Para { padding: 16px; text: long; } } }
                ra := VerticalLayout { if true: ParaA { padding: 16px; text: long; } }
                rb := VerticalLayout { if true: ParaB { text: long; } }
                rc := VerticalLayout { if true: ParaC { text: long; } }
                rd := VerticalLayout { if true: ParaC { pad: 16px; text: long; } }
                re := VerticalLayout { ParaA { padding: 16px; text: long; } }
                s1 := VerticalLayout { RectNote { text: long; } }
                s2 := VerticalLayout { if true: RectNote { text: long; } }
                s3 := VerticalLayout { for x in [long]: RectNote { text: x; } }
                n0 := SRow { label: "x"; hint: long; VerticalLayout { alignment: center; LineEdit { width: 300px; } } }
                n1 := SRow1 { hint: long; VerticalLayout { alignment: center; LineEdit { width: 300px; } } }
                n2 := SRow2 { hint: long; VerticalLayout { alignment: center; LineEdit { width: 300px; } } }
                n3 := SRow { label: "x"; hint: long; LineEdit { width: 300px; } }
                n4 := SRow { label: "x"; hint: long; }
            }
        }
    }
}

fn main() {
    let p = Probe::new().unwrap();
    p.show().unwrap();
    let t = slint::Timer::default();
    let w = p.as_weak();
    t.start(slint::TimerMode::SingleShot, std::time::Duration::from_millis(500), move || {
        let p = w.unwrap();
        println!("A text in layout:      height {:6.1}  preferred {:6.1}", p.get_a_h(), p.get_a_pref());
        println!("B row (no if):         row {:6.1}  hint {:6.1}  hint preferred {:6.1}", p.get_b_row(), p.get_b_hint(), p.get_b_hint_pref());
        println!("C row (hint in if):    row {:6.1}", p.get_c_row());
        println!("D if + HorizontalLayout: box {:6.1}", p.get_d_box());
        println!("E no if + HorizontalLayout: box {:6.1}", p.get_e_box());
        println!("F for + HorizontalLayout: box {:6.1}", p.get_f_box());
        println!("G if + VerticalLayout: box {:6.1}", p.get_g_box());
        println!("H if + HorizontalLayout, no padding: box {:6.1}", p.get_h_box());
        println!("I if + component Note: box {:6.1}", p.get_i_box());
        println!("J if + VerticalLayout {{ Note }}: box {:6.1}", p.get_j_box());
        println!("K for + VerticalLayout {{ Row }}: box {:6.1}", p.get_k_box());
        println!("L for -> Editor -> if -> Row: box {:6.1}", p.get_l_box());
        println!("M settings field shape: box {:6.1}", p.get_m_box());
        println!("M1 if-hint, children no if: {:6.1}", p.get_m1());
        println!("M2 hint no if, children with if: {:6.1}", p.get_m2());
        println!("M3 hint no if, children no if: {:6.1}", p.get_m3());
        println!("M4 for->comp->if->SRow{{LineEdit}}: {:6.1}", p.get_m4());
        println!("M5 for->comp->SRow{{VL{{LineEdit}}}} (no if): {:6.1}", p.get_m5());
        println!("M6 if->SRow{{VL{{LineEdit}}}} on page: {:6.1}", p.get_m6());
        println!("M7 for->SRow{{LineEdit}} on page: {:6.1}", p.get_m7());
        println!("O if->VL{{SRow{{children}}}}: {:6.1}", p.get_o());
        println!("P for->VL{{SRow{{children}}}}: {:6.1}", p.get_pp());
        println!("Q1 if->Para{{padding at use site}}: {:6.1}  (E = 60 is right)", p.get_q1());
        println!("Q2 for->Para: {:6.1}", p.get_q2());
        println!("Q3 if->Wrapper(if->Para): {:6.1}", p.get_q3());
        println!("Q4 if->VL{{if->Para}}: {:6.1}", p.get_q4());
        println!("RA if->ParaA{{padding at use site}}: {:6.1}", p.get_ra());
        println!("RB if->ParaB(font prop, padding inside): {:6.1}", p.get_rb());
        println!("RC if->ParaC(padding prop default): {:6.1}", p.get_rc());
        println!("RD if->ParaC{{pad: 16px}}: {:6.1}", p.get_rd());
        println!("RE no if, ParaA{{padding at use site}}: {:6.1}", p.get_re());
        println!("S1 RectNote, no if: {:6.1}", p.get_s1());
        println!("S2 if->RectNote: {:6.1}", p.get_s2());
        println!("S3 for->RectNote: {:6.1}", p.get_s3());
        println!("N0 SRow + VerticalLayout{{LineEdit}}: {:6.1}", p.get_n0());
        println!("N1 const min-height, if hint: {:6.1}", p.get_n1());
        println!("N2 cond min-height, hint no if: {:6.1}", p.get_n2());
        println!("N3 SRow + LineEdit directly: {:6.1}", p.get_n3());
        println!("N4 SRow without children: {:6.1}", p.get_n4());
        println!("content: preferred {:6.1}  height {:6.1}", p.get_content_pref(), p.get_content_h());
        slint::quit_event_loop().unwrap();
    });
    slint::run_event_loop().unwrap();
}
