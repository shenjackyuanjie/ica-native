use std::collections::{HashMap, HashSet};
use std::time::Instant;

use super::model::*;
use super::ui::{RelationLayoutCache, RelationNetworkState, RelationViewMode};

/// 聚焦视图创建后继续逐帧计算的总步数，约产生 0.3 秒的收敛动画。
const FORCE_LAYOUT_TICKS: u16 = 36;
/// 首次绘制前同步预热的步数，避免把未经计算的初始位置直接闪现在画布上。
const FORCE_WARMUP_TICKS: usize = 4;
/// 每一帧推进的步数；数值过大会减少动画感，过小则需要更长时间才能稳定。
const FORCE_TICKS_PER_FRAME: usize = 2;
/// 每个节点最多抽样检查的斥力邻居数，用固定上限控制大图的计算量。
const MAX_REPULSION_NEIGHBORS: usize = 24;

pub(super) fn visible_relation_node_ids(
    relation_network: &RelationNetworkState,
    query: &str,
) -> Vec<String> {
    let mut ids = match &relation_network.view_mode {
        RelationViewMode::Focused(focused) => {
            relation_visible_ids_from_focus(relation_network, focused, query)
        }
        RelationViewMode::MultiSelectRelationship => {
            let mut ids: HashSet<_> = relation_multi_select_relationship_ids(relation_network)
                .into_iter()
                .collect();
            ids.retain(|id| {
                relation_node_by_id(&relation_network.graph, id).is_some_and(|node| {
                    relation_network.options.allows(node.kind) && node.matches_query(query)
                })
            });
            relation_node_kind_ordered_ids(&relation_network.graph, ids)
        }
        RelationViewMode::Default | RelationViewMode::MultiSelect => {
            relation_visible_ids_default(relation_network, query)
        }
    };
    ids.truncate(relation_view_limit(relation_network));
    ids
}

pub(super) fn relation_view_cache_key(relation_network: &RelationNetworkState, query: &str) -> u64 {
    fn mix(seed: u64, value: u64) -> u64 {
        seed.rotate_left(9).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ value
    }

    let mut option_bits = 0_u64;
    option_bits |= u64::from(relation_network.options.show_self_user);
    option_bits |= u64::from(relation_network.options.show_friends) << 1;
    option_bits |= u64::from(relation_network.options.show_acquaintances) << 2;
    option_bits |= u64::from(relation_network.options.show_strangers) << 3;
    option_bits |= u64::from(relation_network.options.show_groups) << 4;

    let mut hash = mix(0x517c_c1b7_2722_0a95, relation_network.graph_revision);
    hash = mix(hash, stable_relation_hash(query));
    hash = mix(hash, option_bits);
    hash = mix(hash, relation_view_limit(relation_network) as u64);
    hash = mix(hash, relation_drawn_link_limit(relation_network) as u64);
    hash = match &relation_network.view_mode {
        RelationViewMode::Default => mix(hash, 1),
        RelationViewMode::Focused(node_id) => mix(mix(hash, 2), stable_relation_hash(node_id)),
        RelationViewMode::MultiSelect => mix(hash, 3),
        RelationViewMode::MultiSelectRelationship => mix(hash, 4),
    };

    let selected_hash = relation_network
        .selected_node_ids
        .iter()
        .fold(0_u64, |combined, id| combined ^ stable_relation_hash(id));
    hash = mix(hash, selected_hash);
    hash = mix(hash, relation_network.selected_node_ids.len() as u64);
    hash | 1
}

pub(super) fn build_relation_layout_cache(
    relation_network: &RelationNetworkState,
    view_key: u64,
    visible_ids: Vec<String>,
) -> RelationLayoutCache {
    let started_at = Instant::now();
    let visible_set: HashSet<&str> = visible_ids.iter().map(String::as_str).collect();
    let visible_node_indices = visible_ids
        .iter()
        .filter_map(|id| relation_network.graph.node_index.get(id).copied())
        .collect();
    let focused = relation_focused_node_id(relation_network);
    let focus_neighbors = focused.map(|focused| {
        relation_neighbors(&relation_network.graph, focused)
            .into_iter()
            .collect::<HashSet<_>>()
    });
    let max_links = relation_drawn_link_limit(relation_network);
    let mut visible_link_indices =
        Vec::with_capacity(max_links.min(relation_network.graph.links.len()));
    if max_links > 0 {
        for (index, link) in relation_network.graph.links.iter().enumerate() {
            if !visible_set.contains(link.source.as_str())
                || !visible_set.contains(link.target.as_str())
            {
                continue;
            }
            if let Some(focused) = focused
                && link.source != focused
                && link.target != focused
                && !focus_neighbors.as_ref().is_some_and(|neighbors| {
                    relation_link_visible_for_focus(
                        &relation_network.graph,
                        link,
                        focused,
                        neighbors,
                    )
                })
            {
                continue;
            }
            visible_link_indices.push(index);
            if visible_link_indices.len() >= max_links {
                break;
            }
        }
    }
    let mut unit_positions =
        relation_unit_node_positions(&relation_network.graph, &visible_ids, &visible_link_indices);

    // 普通视图沿用按节点类型和群归属生成的稳定布局；进入聚焦视图后重新散开一跳邻居，
    // 为后续力导向计算提供不重叠、可复现的起点。
    if let Some(focused) = focused {
        seed_focused_relation_positions(&visible_ids, focused, &mut unit_positions);
    }

    let mut cache = RelationLayoutCache {
        view_key,
        visible_ids,
        visible_node_indices,
        visible_link_indices,
        unit_positions,
        velocities: HashMap::new(),
        force_ticks_remaining: focused.map_or(0, |_| FORCE_LAYOUT_TICKS),
    };
    if cache.force_ticks_remaining > 0 {
        // 在缓存交给绘制层前先推进少量步数，使首帧已经具有基本合理的相对位置。
        step_relation_force_layout(
            &relation_network.graph,
            &mut cache,
            focused,
            FORCE_WARMUP_TICKS,
        );
    }
    tracing::debug!(
        visible_nodes = cache.visible_node_indices.len(),
        visible_links = cache.visible_link_indices.len(),
        elapsed_ms = started_at.elapsed().as_millis(),
        "relation network layout cache rebuilt"
    );
    cache
}

/// 推进当前聚焦视图的力导向布局。
///
/// 返回值表示计算结束后是否仍有剩余步数，绘制层据此决定是否继续申请下一帧。
/// 非聚焦视图的剩余步数为零，因此不会引入持续重绘开销。
pub(super) fn advance_relation_force_layout(relation_network: &mut RelationNetworkState) -> bool {
    if relation_network.layout_cache.force_ticks_remaining == 0 {
        return false;
    }

    let focused = relation_focused_node_id(relation_network).map(str::to_owned);
    step_relation_force_layout(
        &relation_network.graph,
        &mut relation_network.layout_cache,
        focused.as_deref(),
        FORCE_TICKS_PER_FRAME,
    );
    relation_network.layout_cache.force_ticks_remaining = relation_network
        .layout_cache
        .force_ticks_remaining
        .saturating_sub(FORCE_TICKS_PER_FRAME as u16);
    relation_network.layout_cache.force_ticks_remaining > 0
}

/// 为聚焦视图生成确定性的“向日葵”初始分布。
///
/// 聚焦节点固定在原点，其余节点沿黄金角螺旋展开。与完全随机的位置相比，这种分布既能
/// 避免大量群成员初始时挤在同一个群锚点附近，也能保证重复打开同一节点时不会整体乱跳。
fn seed_focused_relation_positions(
    visible_ids: &[String],
    focused: &str,
    positions: &mut HashMap<String, egui::Vec2>,
) {
    positions.insert(focused.to_owned(), egui::Vec2::ZERO);
    let neighbor_count = visible_ids
        .iter()
        .filter(|id| id.as_str() != focused)
        .count();
    if neighbor_count == 0 {
        return;
    }

    // 使用聚焦节点 ID 计算相位，使不同节点的关系网具有不同朝向，同时保持结果可复现。
    let golden_angle = std::f32::consts::PI * (3.0 - 5.0_f32.sqrt());
    let phase = stable_relation_unit_pair(focused).0 * std::f32::consts::TAU;
    let mut index = 0usize;
    for node_id in visible_ids {
        if node_id == focused {
            continue;
        }
        let progress = (index as f32 + 0.75) / neighbor_count as f32;
        let angle = index as f32 * golden_angle + phase;
        let radius = 0.18 + 0.70 * progress.sqrt();
        positions.insert(
            node_id.clone(),
            egui::vec2(angle.cos(), angle.sin()) * radius,
        );
        index += 1;
    }
}

/// 执行若干次轻量力导向迭代。
///
/// 模型由边的弹簧力、节点间的近距离斥力、指向画布中心的弱引力和速度阻尼组成。
/// 聚焦节点始终固定在原点。为兼顾最多数千节点的聚焦视图，斥力使用确定性抽样，
/// 将每一步的复杂度控制在近似 `O(节点数 + 边数)`，而不是完整两两计算的 `O(节点数²)`。
fn step_relation_force_layout(
    graph: &RelationGraph,
    cache: &mut RelationLayoutCache,
    focused: Option<&str>,
    iterations: usize,
) {
    let node_count = cache.visible_ids.len();
    if node_count < 2 {
        return;
    }

    // 力计算使用紧凑的数组槽位；先建立节点 ID 到槽位的映射，再把可见边转换为槽位对。
    // 这样内层迭代无需反复查询图中的字符串索引。
    let id_to_slot: HashMap<&str, usize> = cache
        .visible_ids
        .iter()
        .enumerate()
        .map(|(slot, id)| (id.as_str(), slot))
        .collect();
    let edges: Vec<(usize, usize)> = cache
        .visible_link_indices
        .iter()
        .filter_map(|&link_index| {
            let link = graph.links.get(link_index)?;
            Some((
                *id_to_slot.get(link.source.as_str())?,
                *id_to_slot.get(link.target.as_str())?,
            ))
        })
        .collect();
    let focused_slot = focused.and_then(|id| id_to_slot.get(id).copied());
    // 位置与速度在一次绘制中连续推进多步，结束后再统一写回缓存，减少 HashMap 访问。
    let mut positions: Vec<_> = cache
        .visible_ids
        .iter()
        .map(|id| cache.unit_positions.get(id).copied().unwrap_or_default())
        .collect();
    let mut velocities: Vec<_> = cache
        .visible_ids
        .iter()
        .map(|id| cache.velocities.get(id).copied().unwrap_or_default())
        .collect();

    // 节点越多，弹簧目标长度越大，给高出度的中心节点留出足够环形空间。
    // 所有长度都采用画布归一化坐标，与窗口实际像素尺寸无关。
    let rest_length = match node_count {
        0..=80 => 0.38,
        81..=400 => 0.50,
        401..=1_500 => 0.64,
        _ => 0.74,
    };
    // 大图适当缩小斥力作用半径，避免密集场景中一次迭代累积过大的合力。
    let repulsion_radius = match node_count {
        0..=100 => 0.16,
        101..=500 => 0.11,
        501..=2_000 => 0.075,
        _ => 0.05,
    };

    for _ in 0..iterations {
        let mut forces = vec![egui::Vec2::ZERO; node_count];

        // 把每条关系边视为弹簧，使相连节点趋向目标距离。单边作用力设有上限，
        // 防止高出度中心在某一步积累过大的合力，把周围节点直接甩出画布。
        for &(source, target) in &edges {
            let delta = positions[target] - positions[source];
            let distance = delta.length().max(0.0001);
            let direction = delta / distance;
            let spring = ((distance - rest_length) * 0.055).clamp(-0.018, 0.018);
            forces[source] += direction * spring;
            forces[target] -= direction * spring;
        }

        // 按固定步长为每个节点抽样一组其他节点，只在距离过近时施加双向斥力。
        // 抽样完全由节点数量和槽位决定，因此布局可复现，同时避免 6,000 节点时的两两计算。
        let stride = (node_count / MAX_REPULSION_NEIGHBORS).max(1);
        for left in 0..node_count {
            let mut checked = 0usize;
            let mut right = (left + 1) % node_count;
            while checked < MAX_REPULSION_NEIGHBORS && right != left {
                let delta = positions[right] - positions[left];
                let distance_sq = delta.length_sq().max(0.000_001);
                if distance_sq < repulsion_radius * repulsion_radius {
                    let distance = distance_sq.sqrt();
                    let direction = if distance > 0.0001 {
                        delta / distance
                    } else {
                        // 两个节点完全重合时无法从位置得到方向，改用槽位生成稳定的分离方向。
                        let angle = (left as f32 * 1.618 + right as f32) * 2.399_963;
                        egui::vec2(angle.cos(), angle.sin())
                    };
                    let strength = ((repulsion_radius - distance) * 0.16).min(0.012);
                    forces[left] -= direction * strength;
                    forces[right] += direction * strength;
                }
                right = (right + stride) % node_count;
                checked += 1;
            }
        }

        // 半隐式地更新速度和位置：弱中心引力防止图形漂走，阻尼帮助布局逐渐稳定，
        // 速度与画布半径限制则避免某一帧位移过大或节点跑到可视范围外。
        for slot in 0..node_count {
            if Some(slot) == focused_slot {
                // 聚焦节点是当前关系网的视觉锚点，始终固定在画布中心。
                positions[slot] = egui::Vec2::ZERO;
                velocities[slot] = egui::Vec2::ZERO;
                continue;
            }
            forces[slot] -= positions[slot] * 0.006;
            velocities[slot] = (velocities[slot] + forces[slot]) * 0.82;
            let speed = velocities[slot].length();
            if speed > 0.035 {
                velocities[slot] *= 0.035 / speed;
            }
            positions[slot] += velocities[slot];
            let radius = positions[slot].length();
            if radius > 0.98 {
                positions[slot] *= 0.98 / radius;
                velocities[slot] *= 0.45;
            }
        }
    }

    // 保存本次计算结果，下一帧从当前速度继续演化，而不是重新开始。
    for (slot, id) in cache.visible_ids.iter().enumerate() {
        cache.unit_positions.insert(id.clone(), positions[slot]);
        cache.velocities.insert(id.clone(), velocities[slot]);
    }
}

fn relation_focused_node_id(relation_network: &RelationNetworkState) -> Option<&str> {
    match &relation_network.view_mode {
        RelationViewMode::Focused(node_id) => Some(node_id.as_str()),
        _ => relation_network.focused_node_id.as_deref(),
    }
}

fn relation_multi_select_relationship_ids(relation_network: &RelationNetworkState) -> Vec<String> {
    if relation_network.selected_node_ids.is_empty() {
        return Vec::new();
    }

    let mut visible_ids = relation_network.selected_node_ids.clone();
    let selected_groups: HashSet<_> = relation_network
        .selected_node_ids
        .iter()
        .filter(|id| {
            relation_node_by_id(&relation_network.graph, id)
                .is_some_and(|node| node.kind == RelationNodeKind::Group)
        })
        .cloned()
        .collect();
    let selected_non_groups: HashSet<_> = relation_network
        .selected_node_ids
        .iter()
        .filter(|id| !selected_groups.contains(*id))
        .cloned()
        .collect();

    if selected_non_groups.len() >= 2 {
        for link in &relation_network.graph.links {
            let source_node = relation_node_by_id(&relation_network.graph, &link.source);
            let target_node = relation_node_by_id(&relation_network.graph, &link.target);
            if source_node.is_some_and(|node| node.kind == RelationNodeKind::Group)
                && selected_non_groups.contains(&link.target)
            {
                visible_ids.insert(link.source.clone());
            } else if target_node.is_some_and(|node| node.kind == RelationNodeKind::Group)
                && selected_non_groups.contains(&link.source)
            {
                visible_ids.insert(link.target.clone());
            }
        }
    }

    if selected_groups.len() >= 2 {
        let mut member_group_counts: HashMap<String, usize> = HashMap::new();
        for link in &relation_network.graph.links {
            let member_id = if selected_groups.contains(&link.source) {
                &link.target
            } else if selected_groups.contains(&link.target) {
                &link.source
            } else {
                continue;
            };
            let Some(member_node) = relation_node_by_id(&relation_network.graph, member_id) else {
                continue;
            };
            if member_node.kind == RelationNodeKind::Group
                || !relation_network.options.allows(member_node.kind)
            {
                continue;
            }
            *member_group_counts.entry(member_id.clone()).or_default() += 1;
        }
        for (member_id, group_count) in member_group_counts {
            if group_count >= 2 {
                visible_ids.insert(member_id);
            }
        }
    }

    if relation_network.selected_node_ids.len() > 1
        && let Some(self_node) = relation_network
            .graph
            .nodes
            .iter()
            .find(|node| node.kind == RelationNodeKind::SelfUser)
    {
        visible_ids.insert(self_node.id.clone());
    }

    relation_node_kind_ordered_ids(&relation_network.graph, visible_ids)
}

pub(super) fn relation_node_by_id<'a>(
    graph: &'a RelationGraph,
    id: &str,
) -> Option<&'a RelationNode> {
    graph
        .node_index
        .get(id)
        .and_then(|index| graph.nodes.get(*index))
}

fn relation_link_visible_for_focus(
    graph: &RelationGraph,
    link: &RelationLink,
    focused: &str,
    focus_neighbors: &HashSet<&str>,
) -> bool {
    link.source == focused
        || link.target == focused
        || (focus_neighbors.contains(link.source.as_str())
            && focus_neighbors.contains(link.target.as_str())
            && relation_node_by_id(graph, &link.source)
                .zip(relation_node_by_id(graph, &link.target))
                .is_some_and(|(source, target)| {
                    source.kind == RelationNodeKind::Group || target.kind == RelationNodeKind::Group
                }))
}

fn relation_node_kind_ordered_ids(graph: &RelationGraph, ids: HashSet<String>) -> Vec<String> {
    let mut ids: Vec<_> = ids.into_iter().collect();
    ids.sort_by(|left, right| {
        match (
            relation_node_by_id(graph, left),
            relation_node_by_id(graph, right),
        ) {
            (Some(left_node), Some(right_node)) => node_kind_order(left_node.kind)
                .cmp(&node_kind_order(right_node.kind))
                .then_with(|| right_node.value.cmp(&left_node.value))
                .then_with(|| left_node.name.cmp(&right_node.name)),
            _ => left.cmp(right),
        }
    });
    ids
}

fn relation_visible_ids_default(
    relation_network: &RelationNetworkState,
    query: &str,
) -> Vec<String> {
    relation_network
        .graph
        .nodes
        .iter()
        .filter(|node| relation_network.options.allows(node.kind) && node.matches_query(query))
        .take(relation_view_limit(relation_network))
        .map(|node| node.id.clone())
        .collect()
}

fn relation_visible_ids_from_focus(
    relation_network: &RelationNetworkState,
    focused: &str,
    query: &str,
) -> Vec<String> {
    let limit = relation_view_limit(relation_network);
    if limit == 0 {
        return Vec::new();
    }
    let neighbors: HashSet<_> = relation_neighbors(&relation_network.graph, focused)
        .into_iter()
        .collect();
    let mut visible_ids = Vec::with_capacity(limit.min(neighbors.len() + 1));
    // 聚焦节点必须优先占用一个名额，否则邻居数超过上限时，它可能因为图中的节点顺序靠后
    // 而被 `take` 截掉，最终既没有中心节点，也无法正确固定力导向布局的原点。
    if relation_node_by_id(&relation_network.graph, focused).is_some() {
        visible_ids.push(focused.to_owned());
    }
    visible_ids.extend(
        relation_network
            .graph
            .nodes
            .iter()
            .filter(|node| {
                node.id != focused
                    && neighbors.contains(node.id.as_str())
                    && relation_network.options.allows(node.kind)
                    && node.matches_query(query)
            })
            .take(limit.saturating_sub(visible_ids.len()))
            .map(|node| node.id.clone()),
    );
    visible_ids
}

fn relation_view_limit(relation_network: &RelationNetworkState) -> usize {
    if matches!(relation_network.view_mode, RelationViewMode::Focused(_)) {
        relation_network.render_setting.max_visible_nodes_focused
    } else {
        relation_network.render_setting.max_visible_nodes
    }
}

fn relation_drawn_link_limit(relation_network: &RelationNetworkState) -> usize {
    if matches!(relation_network.view_mode, RelationViewMode::Focused(_)) {
        relation_network.render_setting.max_drawn_links_focused
    } else {
        relation_network.render_setting.max_drawn_links
    }
}

fn relation_neighbors<'a>(graph: &'a RelationGraph, node_id: &str) -> Vec<&'a str> {
    graph
        .links
        .iter()
        .filter_map(|link| {
            if link.source == node_id {
                Some(link.target.as_str())
            } else if link.target == node_id {
                Some(link.source.as_str())
            } else {
                None
            }
        })
        .collect()
}

pub(super) fn relation_unit_node_positions(
    graph: &RelationGraph,
    visible_ids: &[String],
    visible_link_indices: &[usize],
) -> HashMap<String, egui::Vec2> {
    let mut unit_positions = HashMap::with_capacity(visible_ids.len());
    if visible_ids.len() == 1 {
        return HashMap::from([(visible_ids[0].clone(), egui::Vec2::ZERO)]);
    }

    let visible_nodes: Vec<_> = visible_ids
        .iter()
        .filter_map(|id| relation_node_by_id(graph, id))
        .collect();
    let node_kinds: HashMap<_, _> = visible_nodes
        .iter()
        .map(|node| (node.id.as_str(), node.kind))
        .collect();

    let groups: Vec<_> = visible_nodes
        .iter()
        .copied()
        .filter(|node| node.kind == RelationNodeKind::Group)
        .collect();
    let golden_angle = std::f32::consts::PI * (3.0 - 5.0_f32.sqrt());
    for (index, node) in groups.iter().enumerate() {
        let position = match groups.len() {
            0 => egui::Vec2::ZERO,
            1 => egui::vec2(0.36, 0.0),
            2..=10 => {
                let angle = index as f32 / groups.len() as f32 * std::f32::consts::TAU - 0.35;
                egui::vec2(angle.cos(), angle.sin()) * 0.76
            }
            count => {
                let progress = (index as f32 + 0.65) / count as f32;
                let radius = 0.56 + 0.38 * progress.sqrt();
                let angle = index as f32 * golden_angle - 0.45;
                egui::vec2(angle.cos(), angle.sin()) * radius
            }
        };
        unit_positions.insert(node.id.clone(), position);
    }

    let mut group_anchors: HashMap<&str, (egui::Vec2, usize)> = HashMap::new();
    for &link_index in visible_link_indices {
        let Some(link) = graph.links.get(link_index) else {
            continue;
        };

        let source_kind = node_kinds.get(link.source.as_str()).copied();
        let target_kind = node_kinds.get(link.target.as_str()).copied();
        let (user_id, group_id) = match (source_kind, target_kind) {
            (Some(RelationNodeKind::Group), Some(target_kind))
                if target_kind != RelationNodeKind::Group =>
            {
                (link.target.as_str(), link.source.as_str())
            }
            (Some(source_kind), Some(RelationNodeKind::Group))
                if source_kind != RelationNodeKind::Group =>
            {
                (link.source.as_str(), link.target.as_str())
            }
            _ => continue,
        };
        let Some(group_position) = unit_positions.get(group_id).copied() else {
            continue;
        };
        let anchor = group_anchors
            .entry(user_id)
            .or_insert((egui::Vec2::ZERO, 0));
        anchor.0 += group_position;
        anchor.1 += 1;
    }

    let unanchored_count = visible_nodes
        .iter()
        .filter(|node| {
            node.kind != RelationNodeKind::Group
                && node.kind != RelationNodeKind::SelfUser
                && !group_anchors.contains_key(node.id.as_str())
        })
        .count()
        .max(1);
    let mut unanchored_index = 0usize;
    for node in visible_nodes {
        if node.kind == RelationNodeKind::Group {
            continue;
        }
        if node.kind == RelationNodeKind::SelfUser {
            unit_positions.insert(node.id.clone(), egui::Vec2::ZERO);
            continue;
        }

        let position = if let Some((sum, count)) = group_anchors.get(node.id.as_str()) {
            let anchor = *sum / *count as f32;
            let (angle_unit, radius_unit) = stable_relation_unit_pair(&node.id);
            let cluster_radius = if *count > 1 { 0.075 } else { 0.13 };
            let radius = cluster_radius * (0.28 + radius_unit * 0.72);
            let angle = angle_unit * std::f32::consts::TAU;
            anchor + egui::vec2(angle.cos(), angle.sin()) * radius
        } else {
            let progress = (unanchored_index as f32 + 0.5) / unanchored_count as f32;
            let angle = unanchored_index as f32 * golden_angle + 0.6;
            unanchored_index += 1;
            let outer_radius = if groups.is_empty() { 0.90 } else { 0.52 };
            egui::vec2(angle.cos(), angle.sin()) * (0.04 + outer_radius * progress.sqrt())
        };
        unit_positions.insert(
            node.id.clone(),
            egui::vec2(position.x.clamp(-0.98, 0.98), position.y.clamp(-0.98, 0.98)),
        );
    }

    unit_positions
}

#[derive(Clone, Copy)]
pub(super) struct RelationCanvasTransform {
    center: egui::Pos2,
    scale: f32,
}

impl RelationCanvasTransform {
    pub(super) fn new(rect: egui::Rect, zoom: f32, pan: egui::Vec2) -> Self {
        let usable_rect = rect.shrink2(egui::vec2(54.0, 54.0));
        Self {
            center: usable_rect.center() + pan,
            scale: usable_rect.width().min(usable_rect.height()).max(1.0) * 0.48 * zoom,
        }
    }

    pub(super) fn position(self, unit_position: egui::Vec2) -> egui::Pos2 {
        egui::pos2(
            self.center.x + unit_position.x * self.scale,
            self.center.y + unit_position.y * self.scale,
        )
    }
}

fn stable_relation_unit_pair(value: &str) -> (f32, f32) {
    let hash = stable_relation_hash(value);
    let mixed = hash ^ hash.rotate_left(21).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    let first = (hash & 0xffff_ffff) as f32 / u32::MAX as f32;
    let second = (mixed & 0xffff_ffff) as f32 / u32::MAX as f32;
    (first, second)
}

fn stable_relation_hash(value: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}
