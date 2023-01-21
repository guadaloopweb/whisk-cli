use chrono::prelude::*;
use crossterm::{
    event::{self, Event as CEvent, KeyCode},
    terminal::{disable_raw_mode, enable_raw_mode},
};
use dirs::home_dir;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use std::{fs::{self, File}, path::{PathBuf, Path}, sync::Arc};
use std::io;
use std::io::prelude::*;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use thiserror::Error;
use tui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Span, Spans},
    widgets::{
        Block, BorderType, Borders, Cell, List, ListItem, ListState, Paragraph, Row, Table, Tabs,
    },
    Terminal,
};

const DB_PATH: &str = "/.config/whisk";

#[derive(Error, Debug)]
pub enum Error {
    #[error("error reading the DB file: {0}")]
    ReadDBError(#[from] io::Error),
    #[error("error parsing the DB file: {0}")]
    ParseDBError(#[from] serde_json::Error),
}

enum Event<I> {
    Input(I),
    Tick,
}

#[derive(Serialize, Deserialize, Clone)]
struct Project {
    id: String,
    name: String,
    directory: String,
    created_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug)]
enum MenuItem {
    Home,
    Projects,
}

impl From<MenuItem> for usize {
    fn from(input: MenuItem) -> usize {
        match input {
            MenuItem::Home => 0,
            MenuItem::Projects => 1,
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode().expect("can run in raw mode");

    let (tx, rx) = mpsc::channel();
    let tick_rate = Duration::from_millis(200);
    thread::spawn(move || {
        let mut last_tick = Instant::now();
        loop {
            let timeout = tick_rate
                .checked_sub(last_tick.elapsed())
                .unwrap_or_else(|| Duration::from_secs(0));

            if event::poll(timeout).expect("poll works") {
                if let CEvent::Key(key) = event::read().expect("can read events") {
                    tx.send(Event::Input(key)).expect("can send events");
                }
            }

            if last_tick.elapsed() >= tick_rate {
                if let Ok(_) = tx.send(Event::Tick) {
                    last_tick = Instant::now();
                }
            }
        }
    });

    let stdout = io::stdout();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let menu_titles = vec!["Home", "Projects", "Add", "Delete", "Quit"];
    let mut active_menu_item = MenuItem::Home;
    let mut project_list_state = ListState::default();
    project_list_state.select(Some(0));

    loop {
        terminal.draw(|rect| {
            let size = rect.size();
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(2)
                .constraints(
                    [
                        Constraint::Length(3),
                        Constraint::Min(2),
                        Constraint::Length(3),
                    ]
                    .as_ref(),
                )
                .split(size);

            let menu = menu_titles
                .iter()
                .map(|t| {
                    let (first, rest) = t.split_at(1);
                    Spans::from(vec![
                        Span::styled(
                            first,
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::UNDERLINED),
                        ),
                        Span::styled(rest, Style::default().fg(Color::White)),
                    ])
                })
                .collect();

            let tabs = Tabs::new(menu)
                .select(active_menu_item.into())
                .block(Block::default().title("Menu").borders(Borders::ALL))
                .style(Style::default().fg(Color::White))
                .highlight_style(Style::default().fg(Color::Yellow))
                .divider(Span::raw("|"));

            rect.render_widget(tabs, chunks[0]);
            match active_menu_item {
                MenuItem::Home => rect.render_widget(render_home(), chunks[1]),
                MenuItem::Projects => {
                    let projects_chunks = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints(
                            [Constraint::Percentage(20), Constraint::Percentage(80)].as_ref(),
                        )
                        .split(chunks[1]);
                    let (left, right) = render_projects(&project_list_state);
                    rect.render_stateful_widget(left, projects_chunks[0], &mut project_list_state);
                    rect.render_widget(right.unwrap(), projects_chunks[1]);
                }
            }
        })?;

        match rx.recv()? {
            Event::Input(event) => match event.code {
                KeyCode::Char('q') => {
                    disable_raw_mode()?;
                    terminal.show_cursor()?;
                    break;
                }
                KeyCode::Char('h') => active_menu_item = MenuItem::Home,
                KeyCode::Char('p') => active_menu_item = MenuItem::Projects,
                KeyCode::Char('a') => {
                    match xplr::runner::runner().and_then(|a| a.run()) {
                        Ok(Some(out)) => {
                            let project_name = out
                                .split('/')
                                .next_back()
                                .expect("There is a project name");

                            add_project_to_db(project_name.to_string(), out.to_string()).expect("can add new project");
                        },
                        Ok(None) => {}
                        Err(err) => {
                            if !err.to_string().is_empty() {
                                eprintln!("error: {}", err);
                            };

                            std::process::exit(1);
                        }
                    }
                }
                KeyCode::Char('d') => {
                    remove_project_at_index(&mut project_list_state).expect("can remove project");
                }
                KeyCode::Down => {
                    if let Some(selected) = project_list_state.selected() {
                        let amount_projects = read_db().expect("can fetch project list").len();
                        if selected >= amount_projects - 1 {
                            project_list_state.select(Some(0));
                        } else {
                            project_list_state.select(Some(selected + 1));
                        }
                    }
                }
                KeyCode::Up => {
                    if let Some(selected) = project_list_state.selected() {
                        let amount_projects = read_db().expect("can fetch project list").len();
                        if selected > 0 {
                            project_list_state.select(Some(selected - 1));
                        } else {
                            project_list_state.select(Some(amount_projects - 1));
                        }
                    }
                }
                _ => {}
            },
            Event::Tick => {}
        }
    }

    Ok(())
}

fn render_home<'a>() -> Paragraph<'a> {
    let home = Paragraph::new(vec![
        Spans::from(vec![Span::raw("")]),
        Spans::from(vec![Span::raw("Welcome")]),
        Spans::from(vec![Span::raw("")]),
        Spans::from(vec![Span::raw("to")]),
        Spans::from(vec![Span::raw("")]),
        Spans::from(vec![Span::styled(
            "whisk-CLI",
            Style::default().fg(Color::LightBlue),
        )]),
        Spans::from(vec![Span::raw("")]),
        Spans::from(vec![Span::raw("Press 'p' to access projects, 'a' to add a new project and 'd' to delete the currently selected project.")]),
    ])
    .alignment(Alignment::Center)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::White))
            .title("Home")
            .border_type(BorderType::Plain),
    );
    home
}

fn render_projects<'a>(project_list_state: &ListState) -> (List<'a>, Option<Table<'a>>) {
    let projects = Block::default()
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::White))
        .title("Projects")
        .border_type(BorderType::Plain);

    let project_list = read_db().expect("can fetch project list");

    let items: Vec<_> = project_list
        .iter()
        .map(|project| {
            ListItem::new(Spans::from(vec![Span::styled(
                    project.name.clone(),
                Style::default(),
            )]))
        })
        .collect();

    let list = List::new(items).block(projects).highlight_style(
            Style::default()
            .bg(Color::Yellow)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD),
    );

    // Display selected project if there's any selected
    let selected_project_id = project_list_state.selected();

    if selected_project_id == None || project_list.len() == 0 {
        let project_detail = Some(Table::new(vec![]).block(
                Block::default()
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::White))
                .title("No project selected")
                .border_type(BorderType::Plain),
        ));

        (list, project_detail)
    } else {
        let selected_project = project_list
            .get(selected_project_id.expect("there is a selected project"))
            .unwrap()
            .clone();

        let project_detail = Table::new(vec![Row::new(vec![
            Cell::from(Span::raw(selected_project.id.to_string())),
            Cell::from(Span::raw(selected_project.name)),
            Cell::from(Span::raw(selected_project.directory)),
            Cell::from(Span::raw(selected_project.created_at.to_string())),
        ])])
        .header(Row::new(vec![
            Cell::from(Span::styled(
                    "ID",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Cell::from(Span::styled(
                    "Name",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Cell::from(Span::styled(
                    "Directory",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Cell::from(Span::styled(
                    "Created At",
                Style::default().add_modifier(Modifier::BOLD),
            )),
        ]))
        .block(
                Block::default()
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::White))
                .title("Detail")
                .border_type(BorderType::Plain),
        )
        .widths(&[
            Constraint::Percentage(25),
            Constraint::Percentage(15),
            Constraint::Percentage(50),
            Constraint::Percentage(20),
            ]);

        (list, Some(project_detail))
    }
}

fn get_db_path() -> Arc<String> {
    let home_dir = home_dir().unwrap();
    let db_path: String = home_dir.to_str().unwrap().to_string() + DB_PATH;
    let db_file = db_path.to_owned() + "/db.json";

    fs::create_dir_all(db_path);
    if !Path::new(db_file.as_str()).exists() {
        let mut file = File::create(db_file.as_str()).expect("DB file created");
        file.write_all(b"[]");
    }

    let arc = Arc::new(db_file);

    arc.clone()
}

fn read_db() -> Result<Vec<Project>, Error> {
    let db_content = fs::read_to_string(get_db_path().to_string())?;
    let parsed: Vec<Project> = serde_json::from_str(&db_content)?;
    Ok(parsed)
}

fn add_project_to_db(project_name: String, directory: String) -> Result<Vec<Project>, Error> {
    let db_content = fs::read_to_string(get_db_path().to_string())?;
    let mut parsed: Vec<Project> = serde_json::from_str(&db_content)?;

    let new_project = Project {
        id: Uuid::new_v4().to_string(),
        name: project_name,
        directory: directory,
        created_at: Utc::now(),
    };

    parsed.push(new_project);
    fs::write(get_db_path().to_string(), &serde_json::to_vec(&parsed)?)?;
    Ok(parsed)
}

fn remove_project_at_index(project_list_state: &mut ListState) -> Result<(), Error> {
    if let Some(selected) = project_list_state.selected() {
        let db_content = fs::read_to_string(get_db_path().to_string())?;
        let mut parsed: Vec<Project> = serde_json::from_str(&db_content)?;
        parsed.remove(selected);
        fs::write(get_db_path().to_string(), &serde_json::to_vec(&parsed)?)?;
        // let amount_projects = read_db().expect("can fetch project list").len();
        if selected > 0 {
            project_list_state.select(Some(selected - 1));
        } else {
            project_list_state.select(Some(0));
        }
    }
    Ok(())
}