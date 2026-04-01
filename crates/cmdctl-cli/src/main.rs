use cmdctl_daemon::client::DaemonClient;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("list");

    let mut client = match DaemonClient::connect() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Cannot connect to CMDCTL daemon: {}", e);
            eprintln!("Start CMDCTL first to launch the daemon.");
            std::process::exit(1);
        }
    };

    match cmd {
        "list" | "ls" => {
            match client.list_sessions() {
                Ok(sessions) => {
                    if sessions.is_empty() {
                        println!("No active sessions.");
                    } else {
                        println!("{:<12} {:<25} {:<8} {}", "ID", "NAME", "TYPE", "STATUS");
                        for s in sessions {
                            println!("{:<12} {:<25} {:<8} {}", s.id, s.name, s.agent_type, s.status);
                        }
                    }
                }
                Err(e) => eprintln!("Error: {}", e),
            }
        }
        "kill" => {
            let Some(id) = args.get(2) else {
                eprintln!("Usage: cmdctl-cli kill <session-id>");
                std::process::exit(1);
            };
            match client.kill_session(id) {
                Ok(()) => println!("Killed session {}", id),
                Err(e) => eprintln!("Error: {}", e),
            }
        }
        "dump" => {
            let Some(id) = args.get(2) else {
                eprintln!("Usage: cmdctl-cli dump <session-id>");
                std::process::exit(1);
            };
            match client.get_grid(id) {
                Ok(grid) => {
                    let mut rows: std::collections::BTreeMap<u16, Vec<(u16, char)>> = std::collections::BTreeMap::new();
                    for cell in &grid.cells {
                        rows.entry(cell.row).or_default().push((cell.col, cell.ch));
                    }
                    for (row, mut cols) in rows {
                        cols.sort_by_key(|(c, _)| *c);
                        let line: String = cols.iter().map(|(_, ch)| ch).collect();
                        println!("{:3}|{}", row, line.trim_end());
                    }
                }
                Err(e) => eprintln!("Error: {}", e),
            }
        }
        "shutdown" => {
            match client.shutdown() {
                Ok(()) => println!("Daemon shutting down."),
                Err(e) => eprintln!("Error: {}", e),
            }
        }

        // -- Knowledge commands --

        "knowledge" | "kb" => {
            let subcmd = args.get(2).map(|s| s.as_str()).unwrap_or("list");
            match subcmd {
                "list" | "ls" => {
                    let scope = args.get(3).map(|s| s.as_str());
                    match client.list_knowledge(scope) {
                        Ok(entries) => {
                            if entries.is_empty() {
                                println!("No knowledge entries.");
                            } else {
                                println!("{:<38} {:<30} {:<12} {}", "ID", "TITLE", "SCOPE", "TAGS");
                                for e in entries {
                                    let title = if e.title.len() > 28 {
                                        format!("{}...", &e.title[..25])
                                    } else {
                                        e.title.clone()
                                    };
                                    let scope = if e.scope.len() > 10 {
                                        format!("{}...", &e.scope[..7])
                                    } else {
                                        e.scope.clone()
                                    };
                                    println!("{:<38} {:<30} {:<12} {}", e.id, title, scope, e.tags);
                                }
                            }
                        }
                        Err(e) => eprintln!("Error: {}", e),
                    }
                }
                "search" => {
                    let Some(query) = args.get(3) else {
                        eprintln!("Usage: cmdctl-cli knowledge search <query>");
                        std::process::exit(1);
                    };
                    let scope = args.get(4).map(|s| s.as_str());
                    match client.search_knowledge(query, scope) {
                        Ok(entries) => {
                            if entries.is_empty() {
                                println!("No results for '{}'.", query);
                            } else {
                                println!("Found {} results:\n", entries.len());
                                for e in entries {
                                    println!("  [{}] {} (scope: {})", e.id, e.title, e.scope);
                                    if !e.tags.is_empty() {
                                        println!("    tags: {}", e.tags);
                                    }
                                    let preview: String = e.content.chars().take(100).collect();
                                    let preview = preview.replace('\n', " ");
                                    println!("    {}", preview);
                                    println!();
                                }
                            }
                        }
                        Err(e) => eprintln!("Error: {}", e),
                    }
                }
                "add" => {
                    // cmdctl-cli knowledge add <title> <scope> [tags]
                    // Content is read from stdin.
                    let Some(title) = args.get(3) else {
                        eprintln!("Usage: cmdctl-cli knowledge add <title> <scope> [tags]");
                        std::process::exit(1);
                    };
                    let scope = args.get(4).map(|s| s.as_str()).unwrap_or("global");
                    let tags = args.get(5).map(|s| s.as_str()).unwrap_or("");

                    eprintln!("Enter content (Ctrl+D when done):");
                    let mut content = String::new();
                    use std::io::Read;
                    if let Err(e) = std::io::stdin().read_to_string(&mut content) {
                        eprintln!("Failed to read stdin: {}", e);
                        std::process::exit(1);
                    }

                    match client.add_knowledge(title, content.trim(), scope, tags) {
                        Ok(id) => println!("Created knowledge entry: {}", id),
                        Err(e) => eprintln!("Error: {}", e),
                    }
                }
                "remove" | "rm" => {
                    let Some(id) = args.get(3) else {
                        eprintln!("Usage: cmdctl-cli knowledge remove <id>");
                        std::process::exit(1);
                    };
                    match client.remove_knowledge(id) {
                        Ok(()) => println!("Removed knowledge entry: {}", id),
                        Err(e) => eprintln!("Error: {}", e),
                    }
                }
                "context" => {
                    let wd = args.get(3)
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| std::env::current_dir()
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_else(|_| ".".to_string()));
                    match client.get_context(&wd) {
                        Ok(ctx) => {
                            if ctx.is_empty() {
                                println!("No context available for {}", wd);
                            } else {
                                println!("{}", ctx);
                            }
                        }
                        Err(e) => eprintln!("Error: {}", e),
                    }
                }
                "summaries" => {
                    let wd = args.get(3).map(|s| s.as_str());
                    match client.list_session_summaries(wd) {
                        Ok(summaries) => {
                            if summaries.is_empty() {
                                println!("No session summaries.");
                            } else {
                                for s in summaries {
                                    println!("--- {} ({}) ---", s.session_name, s.created_at);
                                    println!("  Dir: {}", s.working_dir);
                                    if !s.summary.is_empty() {
                                        println!("  Summary: {}", s.summary);
                                    }
                                    if !s.decisions.is_empty() {
                                        println!("  Decisions: {}", s.decisions);
                                    }
                                    if !s.unresolved.is_empty() {
                                        println!("  Unresolved: {}", s.unresolved);
                                    }
                                    println!();
                                }
                            }
                        }
                        Err(e) => eprintln!("Error: {}", e),
                    }
                }
                _ => {
                    eprintln!("Usage: cmdctl-cli knowledge [list|search <query>|add <title> <scope> [tags]|remove <id>|context [dir]|summaries [dir]]");
                }
            }
        }

        _ => {
            eprintln!("Usage: cmdctl-cli [list|kill <id>|dump <id>|shutdown|knowledge <subcmd>]");
        }
    }
}
