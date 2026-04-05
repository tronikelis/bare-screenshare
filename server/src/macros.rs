macro_rules! shadowclone {
    ($($var:ident),+) => {
        $(
            let $var = $var.clone();
        )+
    }
}

pub(crate) use shadowclone;
