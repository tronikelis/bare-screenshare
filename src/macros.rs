macro_rules! clone_expr {
    ($($var:ident),+ => $expr:expr) => {{
        $(
            let $var = $var.clone();
        )+
        $expr
    }};
}

pub(crate) use clone_expr;
