//! Real typos from the owner's own typing, run in BundledOnly mode with no OS checker.

mod common;

use common::fix;

/// (typed, expected, previous word)
const CORPUS: &[(&str, &str, Option<&str>)] = &[
    ("issuses", "issues", None),
    ("aautocorrect", "autocorrect", None),
    ("liek", "like", None),
    ("martking", "marketing", Some("good")),
    ("beginign", "beginning", None),
    ("deciing", "deciding", None),
    ("disctation", "dictation", None),
    ("inserss", "inserts", None),
    ("wouldnt", "wouldn't", None),
    ("teh", "the", None),
    ("recieve", "receive", None),
    ("performatn", "performant", None),
    ("hte", "the", None),
    ("becuase", "because", None),
    ("definately", "definitely", None),
    ("adn", "and", None),
    ("taht", "that", None),
    ("Couls", "Could", None),
    ("tou", "you", Some("if")),
    ("themt", "them", Some("prefer")),
    ("ammoutn", "amount", Some("the")),
    ("typeo", "typo", None),
    ("suggesiton", "suggestion", None),
    ("compatable", "compatible", None),
    ("alredad", "already", None),
];

#[test]
fn real_typo_corpus() {
    let mut right = 0;
    println!("| typed | expected | got |\n|---|---|---|");
    for &(typed, expected, prev) in CORPUS {
        let got = fix(prev, typed);
        let mark = if got == expected { "ok" } else { "MISS" };
        println!("| {typed} | {expected} | {got} {mark} |");
        if got == expected {
            right += 1;
        }
    }
    let pct = right as f32 * 100.0 / CORPUS.len() as f32;
    println!("accuracy: {right}/{} = {pct:.0}%", CORPUS.len());
    assert!(pct >= 85.0, "corpus accuracy {pct:.0}% is below 85%");
}

/// Common English typos (not the owner's), used as a held-out check while tuning.
/// `prev:typed expected`, comma separated.
const COMMON: &str = "thier their,wierd weird,seperate separate,occured occurred,untill until,\
accomodate accommodate,goverment government,tommorow tomorrow,freind friend,beleive believe,\
wich which,jsut just,waht what,whne when,knwo know,abotu about,yuo you,ahve have,mroe more,\
tihs this,somthing something,probaly probably,realy really,thnaks thanks,pleae please,\
becasue because,peopel people,woudl would,shoudl should,cuold could,agian again,\
differnt different,importnat important,thign thing,sutff stuff,actualy actually,\
basicaly basically,definetly definitely,recomend recommend,neccessary necessary,\
begining beginning,enviroment environment,familar familiar,finaly finally,happend happened,\
immediatly immediately,independant independent,knowlege knowledge,lenght length,\
noticable noticeable,occurence occurrence,posession possession,reccomend recommend,\
refered referred,relevent relevant,sentance sentence,succesful successful,suprise surprise,\
tendancy tendency,truely truly,writting writing,youre you're,dont don't,didnt didn't,\
doesnt doesn't,isnt isn't,im I'm,ive I've,thats that's,whats what's,theyre they're,\
wasnt wasn't,hwo how,nto not,gonig going,loking looking,workign working,tryign trying,\
havign having,mkae make,amke make,tiem time,smae same,yaer year,veyr very,tehre there,\
whihc which,wiht with,iwth with,nad and,chnage change,cahnge change,udpate update,\
upadte update,fucntion function,funtion function,retrun return,reutrn return,widht width,\
heigth height,serach search,i:thikn think,the:porblem problem,you:konw know,a:lto lot,\
to:teh the,of:thsi this,it:si is,is:nto not,for:yuor your,in:teh the";

#[test]
fn common_typos() {
    let items: Vec<&str> = COMMON.split(',').collect();
    let mut misses = Vec::new();
    for item in &items {
        let (prev, rest) = match item.split_once(':') {
            Some((p, r)) => (Some(p), r),
            None => (None, *item),
        };
        let (typed, expected) = rest.split_once(' ').unwrap();
        let got = fix(prev, typed);
        if got != expected {
            misses.push(format!("{typed}->{got}"));
        }
    }
    let right = items.len() - misses.len();
    let pct = right as f32 * 100.0 / items.len() as f32;
    println!("common typos: {right}/{} = {pct:.0}%", items.len());
    println!("misses: {}", misses.join(" "));
    assert!(pct >= 85.0, "common typo accuracy {pct:.0}% is below 85%");
}

#[test]
fn missing_space_is_not_fixed_wrongly() {
    let got = fix(None, "Iwant");
    assert!(got == "Iwant" || got == "I want", "Iwant became {got}");
}
