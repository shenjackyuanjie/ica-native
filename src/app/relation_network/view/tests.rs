//! 关系网图谱构建与布局的回归测试。

use super::*;

#[test]
fn group_color_becomes_darker_as_member_count_grows() {
    let small = relation_group_color(Some(10));
    let medium = relation_group_color(Some(1_000));
    let large = relation_group_color(Some(50_000));
    let brightness =
        |color: egui::Color32| u16::from(color.r()) + u16::from(color.g()) + u16::from(color.b());

    assert!(brightness(small) > brightness(medium));
    assert!(brightness(medium) > brightness(large));
    assert_eq!(relation_group_color(None), RelationNodeKind::Group.color());
}

fn test_node(id: &str, kind: RelationNodeKind) -> RelationNode {
    RelationNode {
        id: id.to_string(),
        name: id.to_string(),
        kind,
        value: 1,
        radius: 8.0,
        size_level: 0,
        qq: None,
        group_id: None,
        member_count: None,
        common_group_count: 0,
        role: String::new(),
    }
}

fn test_graph(nodes: Vec<RelationNode>, links: Vec<RelationLink>) -> RelationGraph {
    let node_index = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id.clone(), index))
        .collect();
    let group_node_indices = nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| (node.kind == RelationNodeKind::Group).then_some(index))
        .collect();
    RelationGraph {
        nodes,
        links,
        node_index,
        group_node_indices,
        node_counts: RelationNodeCounts::default(),
        loaded_group_count: 0,
        total_group_count: 0,
    }
}

#[test]
fn overview_limit_keeps_group_backbone_when_user_nodes_exceed_limit() {
    use crate::app::relation_network::layout::{RelationLayoutModel, relation_visible_ids_default};
    use crate::app::relation_network::model::RelationNodeKind;

    let mut nodes = vec![test_node("u:self", RelationNodeKind::SelfUser)];
    // 用默认可见的「共同群好友」充当海量用户节点，避免被默认筛选挡在门外。
    for i in 0..500 {
        nodes.push(test_node(&format!("u:{i}"), RelationNodeKind::Acquaintance));
    }
    for i in 0..20 {
        nodes.push(test_node(&format!("g:{i}"), RelationNodeKind::Group));
    }
    let mut model = RelationLayoutModel::default();
    model.graph = test_graph(nodes, vec![]);
    model.render_setting.max_visible_nodes = 100;

    let visible = relation_visible_ids_default(&model, "");

    // 自己与全部群节点是骨架，必须优先保留，否则画布会退化成没有连线的散点。
    assert!(visible.contains(&"u:self".to_string()));
    for i in 0..20 {
        assert!(
            visible.contains(&format!("g:{i}")),
            "群骨架节点不应被用户节点挤掉"
        );
    }
    assert_eq!(visible.len(), 100);
}

#[test]
fn topology_layout_clusters_member_around_its_group() {
    let graph = test_graph(
        vec![
            test_node("u:1", RelationNodeKind::Friend),
            test_node("g:1", RelationNodeKind::Group),
        ],
        vec![RelationLink {
            source: "g:1".to_string(),
            target: "u:1".to_string(),
        }],
    );
    let visible = vec!["u:1".to_string(), "g:1".to_string()];
    let positions =
        relation_unit_node_positions(&graph, &visible, &[0], 2.40 * RELATION_LAYOUT_SCALE, 0.0);

    let member = positions["u:1"];
    let group = positions["g:1"];
    assert!((member - group).length() <= 0.131 * RELATION_LAYOUT_SCALE);
    assert_eq!(
        positions,
        relation_unit_node_positions(&graph, &visible, &[0], 2.40 * RELATION_LAYOUT_SCALE, 0.0,)
    );
}

#[test]
fn layout_cache_key_changes_with_filters_and_graph_revision() {
    let mut state = RelationNetworkState::default();
    state.replace_graph(test_graph(
        vec![test_node("g:1", RelationNodeKind::Group)],
        Vec::new(),
    ));
    let initial = relation_view_cache_key(&state, "");
    state.options.show_groups = false;
    let filtered = relation_view_cache_key(&state, "");
    assert_ne!(initial, filtered);

    state.options.show_groups = true;
    state.graph_revision = state.graph_revision.wrapping_add(1);
    assert_ne!(initial, relation_view_cache_key(&state, ""));
}

#[test]
fn layout_cache_honors_zero_link_limit() {
    let mut state = RelationNetworkState::default();
    state.render_setting.max_drawn_links = 0;
    state.replace_graph(test_graph(
        vec![
            test_node("u:1", RelationNodeKind::Friend),
            test_node("g:1", RelationNodeKind::Group),
        ],
        vec![RelationLink {
            source: "g:1".to_string(),
            target: "u:1".to_string(),
        }],
    ));
    let visible = vec!["u:1".to_string(), "g:1".to_string()];
    let cache = build_relation_layout_cache(&state, 1, visible);
    assert!(cache.visible_link_indices.is_empty());
}

#[test]
fn canvas_transform_keeps_equal_scale_on_wide_windows() {
    let transform = RelationCanvasTransform::new(
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1_200.0, 500.0)),
        1.0,
        egui::Vec2::ZERO,
        1.20,
    );
    let center = transform.position(egui::Vec2::ZERO);
    let horizontal = transform.position(egui::vec2(1.0, 0.0));
    let vertical = transform.position(egui::vec2(0.0, 1.0));

    assert!(((horizontal - center).length() - (vertical - center).length()).abs() < 0.001);
}

#[test]
fn dense_overview_initializes_groups_outside_friends() {
    let mut nodes = Vec::new();
    let mut visible = Vec::new();
    for index in 0..32 {
        let id = format!("u:{index}");
        nodes.push(test_node(&id, RelationNodeKind::Friend));
        visible.push(id);
    }
    for index in 0..32 {
        let id = format!("g:{index}");
        nodes.push(test_node(&id, RelationNodeKind::Group));
        visible.push(id);
    }
    let graph = test_graph(nodes, Vec::new());
    let positions =
        relation_unit_node_positions(&graph, &visible, &[], 2.40 * RELATION_LAYOUT_SCALE, 0.0);

    let max_friend_radius = positions
        .iter()
        .filter(|(id, _)| id.starts_with("u:"))
        .map(|(_, position)| position.length())
        .fold(0.0_f32, f32::max);
    let max_group_radius = positions
        .iter()
        .filter(|(id, _)| id.starts_with("g:"))
        .map(|(_, position)| position.length())
        .fold(0.0_f32, f32::max);
    assert!(max_group_radius > max_friend_radius + 10.0);
}

#[test]
fn focused_view_keeps_focus_when_node_limit_truncates_neighbors() {
    let mut nodes = (0..8)
        .map(|index| test_node(&format!("u:{index}"), RelationNodeKind::Friend))
        .collect::<Vec<_>>();
    nodes.push(test_node("g:focus", RelationNodeKind::Group));
    let links = (0..8)
        .map(|index| RelationLink {
            source: "g:focus".to_string(),
            target: format!("u:{index}"),
        })
        .collect();
    let mut state = RelationNetworkState::default();
    state.render_setting.max_visible_nodes_focused = 4;
    state.replace_graph(test_graph(nodes, links));
    state.view_mode = RelationViewMode::Focused("g:focus".to_string());

    let visible = visible_relation_node_ids(&state, "");
    assert_eq!(visible.len(), 4);
    assert_eq!(visible[0], "g:focus");
}

#[test]
fn focused_force_layout_anchors_focus_and_spreads_neighbors() {
    let mut nodes = vec![test_node("g:focus", RelationNodeKind::Group)];
    let mut links = Vec::new();
    for index in 0..120 {
        let id = format!("u:{index}");
        nodes.push(test_node(&id, RelationNodeKind::Friend));
        links.push(RelationLink {
            source: "g:focus".to_string(),
            target: id,
        });
    }
    let mut state = RelationNetworkState::default();
    state.replace_graph(test_graph(nodes, links));
    state.view_mode = RelationViewMode::Focused("g:focus".to_string());
    let visible = visible_relation_node_ids(&state, "");
    let cache = build_relation_layout_cache(&state, 1, visible);

    assert!(cache.unit_positions["g:focus"].length() < 0.001);
    let average_neighbor_radius = cache
        .unit_positions
        .iter()
        .filter(|(id, _)| id.as_str() != "g:focus")
        .map(|(_, position)| position.length())
        .sum::<f32>()
        / 120.0;
    assert!(average_neighbor_radius > 0.35);
}

#[test]
fn overview_force_layout_respects_tick_gap_and_settles_into_idle() {
    let mut state = RelationNetworkState::default();
    state.replace_graph(test_graph(
        vec![
            test_node("u:1", RelationNodeKind::Friend),
            test_node("g:1", RelationNodeKind::Group),
        ],
        vec![RelationLink {
            source: "g:1".to_string(),
            target: "u:1".to_string(),
        }],
    ));
    let visible = visible_relation_node_ids(&state, "");
    state.layout_cache = build_relation_layout_cache(&state, 1, visible);

    let now = Instant::now();
    let interval = advance_relation_force_layout(&mut state, now).unwrap();
    let positions_after_first_tick = state.layout_cache.unit_positions.clone();

    // 在计划时间之前发生的额外 UI 帧只能缩短等待时间，不能提前移动节点。
    let early_wait = advance_relation_force_layout(&mut state, now + interval / 2).unwrap();
    assert!(early_wait < interval);
    assert_eq!(
        state.layout_cache.unit_positions,
        positions_after_first_tick
    );

    let next_interval = advance_relation_force_layout(&mut state, now + interval).unwrap();
    assert_ne!(
        state.layout_cache.unit_positions,
        positions_after_first_tick
    );

    // 布局稳定后应停止请求重绘，避免窗口空转烧 CPU。
    let mut tick_at = now + interval + next_interval;
    let mut steps = 0;
    let wait = loop {
        match advance_relation_force_layout(&mut state, tick_at) {
            Some(wait) => {
                tick_at += wait;
                steps += 1;
                assert!(steps < 10_000, "布局迟迟不收敛");
            }
            None => break tick_at,
        }
    };
    // 收敛之后，任何后续调用都继续返回 None，不再安排新的迭代。
    assert!(advance_relation_force_layout(&mut state, wait).is_none());
}

#[test]
fn configured_group_and_friend_lengths_form_separate_layers() {
    let mut state = RelationNetworkState::default();
    state.replace_graph(test_graph(
        vec![
            test_node("u:self", RelationNodeKind::SelfUser),
            test_node("u:friend", RelationNodeKind::Friend),
            test_node("g:1", RelationNodeKind::Group),
        ],
        vec![
            RelationLink {
                source: "u:self".to_string(),
                target: "u:friend".to_string(),
            },
            RelationLink {
                source: "u:self".to_string(),
                target: "g:1".to_string(),
            },
        ],
    ));
    let visible = visible_relation_node_ids(&state, "");
    state.layout_cache = build_relation_layout_cache(&state, 1, visible);

    let mut tick_at = Instant::now();
    for _ in 0..10_000 {
        match advance_relation_force_layout(&mut state, tick_at) {
            Some(wait) => tick_at += wait,
            None => break,
        }
    }

    let friend_radius = state.layout_cache.unit_positions["u:friend"].length();
    let group_radius = state.layout_cache.unit_positions["g:1"].length();
    assert!(group_radius > friend_radius + 0.08);
}

#[test]
fn dense_friends_use_a_wide_radial_band_instead_of_one_outer_ring() {
    let mut nodes = vec![test_node("u:self", RelationNodeKind::SelfUser)];
    let mut links = Vec::new();
    for index in 0..300 {
        let id = format!("u:{index}");
        nodes.push(test_node(&id, RelationNodeKind::Friend));
        links.push(RelationLink {
            source: "u:self".to_string(),
            target: id,
        });
    }
    let mut state = RelationNetworkState::default();
    state.replace_graph(test_graph(nodes, links));
    let visible = visible_relation_node_ids(&state, "");
    state.layout_cache = build_relation_layout_cache(&state, 1, visible);

    let mut tick_at = Instant::now();
    // 推进到布局收敛；收敛后 advance 返回 None，此时再测量最终半径分布。
    for _ in 0..10_000 {
        match advance_relation_force_layout(&mut state, tick_at) {
            Some(wait) => tick_at += wait,
            None => break,
        }
    }

    let mut radii = state
        .layout_cache
        .unit_positions
        .iter()
        .filter(|(id, _)| id.as_str() != "u:self")
        .map(|(_, position)| position.length())
        .collect::<Vec<_>>();
    radii.sort_by(f32::total_cmp);
    let inner_decile = radii[radii.len() / 10];
    let outer_decile = radii[radii.len() * 9 / 10];
    assert!(outer_decile - inner_decile > 0.20);
    assert!(outer_decile < relation_force_layout_max_radius(&state));
}

#[test]
fn dense_groups_use_a_wide_radius_without_hitting_the_layout_boundary() {
    let mut nodes = vec![test_node("u:self", RelationNodeKind::SelfUser)];
    let mut links = Vec::new();
    for index in 0..160 {
        let friend_id = format!("u:{index}");
        nodes.push(test_node(&friend_id, RelationNodeKind::Friend));
        links.push(RelationLink {
            source: "u:self".to_string(),
            target: friend_id,
        });

        let group_id = format!("g:{index}");
        nodes.push(test_node(&group_id, RelationNodeKind::Group));
        links.push(RelationLink {
            source: "u:self".to_string(),
            target: group_id,
        });
    }
    let mut state = RelationNetworkState::default();
    // 与随仓库提供的配置一致：好友长度保持原值，只扩大群边长度。
    state.render_setting.force_friend_link_length = 0.52;
    state.render_setting.force_group_link_length = 2.40;
    state.replace_graph(test_graph(nodes, links));
    let visible = visible_relation_node_ids(&state, "");
    state.layout_cache = build_relation_layout_cache(&state, 1, visible);

    let mut tick_at = Instant::now();
    // 推进到布局收敛；收敛后 advance 返回 None，此时再测量最终半径分布。
    for _ in 0..10_000 {
        match advance_relation_force_layout(&mut state, tick_at) {
            Some(wait) => tick_at += wait,
            None => break,
        }
    }

    let mut friend_radii = Vec::new();
    let mut group_radii = Vec::new();
    for (id, position) in &state.layout_cache.unit_positions {
        if id.starts_with("g:") {
            group_radii.push(position.length());
        } else if id.as_str() != "u:self" {
            friend_radii.push(position.length());
        }
    }
    friend_radii.sort_by(f32::total_cmp);
    group_radii.sort_by(f32::total_cmp);
    let outer_friend = friend_radii[friend_radii.len() * 9 / 10];
    let inner_group = group_radii[group_radii.len() / 10];
    let outer_group = group_radii[group_radii.len() * 9 / 10];
    assert!(outer_group > outer_friend + 10.0);
    assert!(outer_group - inner_group > 8.0);
    assert!(outer_group < relation_force_layout_max_radius(&state));
}

#[test]
fn force_layout_repels_overlapping_spatial_neighbors() {
    let mut state = RelationNetworkState::default();
    state.replace_graph(test_graph(
        vec![
            test_node("u:1", RelationNodeKind::Friend),
            test_node("u:2", RelationNodeKind::Friend),
        ],
        Vec::new(),
    ));
    let visible = visible_relation_node_ids(&state, "");
    state.layout_cache = build_relation_layout_cache(&state, 1, visible);
    state
        .layout_cache
        .unit_positions
        .insert("u:1".to_string(), egui::Vec2::ZERO);
    state
        .layout_cache
        .unit_positions
        .insert("u:2".to_string(), egui::Vec2::ZERO);

    advance_relation_force_layout(&mut state, Instant::now());

    let distance = (state.layout_cache.unit_positions["u:1"]
        - state.layout_cache.unit_positions["u:2"])
        .length();
    assert!(distance > 0.001);
}

#[test]
fn overview_force_layout_keeps_self_user_at_center() {
    let mut state = RelationNetworkState::default();
    state.replace_graph(test_graph(
        vec![
            test_node("u:self", RelationNodeKind::SelfUser),
            test_node("u:friend", RelationNodeKind::Friend),
            test_node("g:1", RelationNodeKind::Group),
        ],
        vec![
            RelationLink {
                source: "g:1".to_string(),
                target: "u:self".to_string(),
            },
            RelationLink {
                source: "g:1".to_string(),
                target: "u:friend".to_string(),
            },
        ],
    ));
    let visible = visible_relation_node_ids(&state, "");
    state.layout_cache = build_relation_layout_cache(&state, 1, visible);

    advance_relation_force_layout(&mut state, Instant::now());

    assert_eq!(
        state.layout_cache.unit_positions["u:self"],
        egui::Vec2::ZERO
    );
}
