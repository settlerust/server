//! Requests run serially; socket bookkeeping may precede a retryable durable commit.
use super::*;
impl Actor {
    pub(super) async fn handle(&mut self, request: Request) {
        match request {
            Request::Activity {
                subject,
                session,
                idle,
                now,
                reply,
            } => {
                let result = if let Some(active) = self.sessions.get_mut(&subject) {
                    if let Some(status) = active.get_mut(&session) {
                        *status = idle;
                        let idle = active.values().all(|idle| *idle);
                        if self
                            .room
                            .member(&subject)
                            .is_some_and(|m| m.idle != idle && m.connected)
                        {
                            commit(
                                &mut self.room,
                                &self.store,
                                &self.events,
                                Input::Activity { subject, idle },
                                now,
                            )
                            .await
                        } else {
                            Ok(())
                        }
                    } else {
                        Ok(())
                    }
                } else {
                    Ok(())
                };
                let _ = reply.send(result);
            }
            Request::DisplayName {
                identity,
                revision,
                now,
                reply,
            } => {
                let result = if (self.room.member(&identity.subject).is_none()
                    && !self.room.display_names.contains_key(&identity.subject))
                    || self
                        .room
                        .display_names
                        .get(&identity.subject)
                        .is_some_and(|name| name.revision >= revision)
                {
                    Ok(())
                } else {
                    commit(
                        &mut self.room,
                        &self.store,
                        &self.events,
                        Input::DisplayName { identity, revision },
                        now,
                    )
                    .await
                };
                let _ = reply.send(result);
            }
            Request::Admission {
                identity,
                request,
                now,
                reply,
            } => {
                let result = if self.room.host != identity.subject {
                    Err(Error::new(
                        ErrorCode::Forbidden,
                        "only the host can update admission",
                    ))
                } else if self.room.admission.as_ref() == Some(&request) {
                    Ok(())
                } else {
                    commit(
                        &mut self.room,
                        &self.store,
                        &self.events,
                        Input::Admission { identity, request },
                        now,
                    )
                    .await
                };
                self.sessions
                    .retain(|subject, _| self.room.member(subject).is_some());
                let _ = reply.send(result);
            }
            Request::Read { reply } => {
                let _ = reply.send(self.room.clone());
            }
            Request::Execute {
                identity,
                request,
                now,
                reply,
            } => {
                let result = execute(
                    &mut self.room,
                    &self.store,
                    &self.events,
                    identity,
                    request,
                    now,
                )
                .await;
                let _ = reply.send(result);
            }
            Request::Presence {
                subject,
                session,
                connected,
                now,
                reply,
            } => {
                if let Err(error) = expire(&mut self.room, &self.store, &self.events, now).await {
                    let _ = reply.send(Err(error));
                    return;
                }
                let active = self.sessions.entry(subject.clone()).or_default();
                if connected {
                    active.insert(session, false);
                } else {
                    active.remove(&session);
                }
                let connected = !active.is_empty();
                let idle = connected && active.values().all(|idle| *idle);
                let result = if self
                    .room
                    .member(&subject)
                    .is_some_and(|m| m.connected == connected)
                {
                    Ok(())
                } else {
                    commit(
                        &mut self.room,
                        &self.store,
                        &self.events,
                        Input::Presence {
                            subject: subject.clone(),
                            connected,
                        },
                        now,
                    )
                    .await
                };
                let result = if result.is_ok()
                    && connected
                    && self.room.member(&subject).is_some_and(|m| m.idle != idle)
                {
                    commit(
                        &mut self.room,
                        &self.store,
                        &self.events,
                        Input::Activity { subject, idle },
                        now,
                    )
                    .await
                } else {
                    result
                };
                let _ = reply.send(result);
            }
            Request::Tick { now, reply } => {
                let mut result = Ok(());
                for subject in self
                    .room
                    .members
                    .iter()
                    .filter(|m| m.computer_policy.is_none())
                    .filter(|m| {
                        (m.seat.is_some()
                            && !m.connected
                            && m.disconnected_at_ms.is_none()
                            && m.resolution.is_none())
                            || m.connected
                                != self
                                    .sessions
                                    .get(&m.identity.subject)
                                    .is_some_and(|s| !s.is_empty())
                    })
                    .map(|m| m.identity.subject.clone())
                    .collect::<Vec<_>>()
                {
                    let connected = self.sessions.get(&subject).is_some_and(|s| !s.is_empty());
                    if let Err(error) = commit(
                        &mut self.room,
                        &self.store,
                        &self.events,
                        Input::Presence { subject, connected },
                        now,
                    )
                    .await
                    {
                        result = Err(error);
                        break;
                    }
                }
                if result.is_ok() {
                    result = expire(&mut self.room, &self.store, &self.events, now).await;
                }
                if result.is_ok()
                    && self.computer.is_none()
                    && let Some((seat, policy, view)) = super::super::expert::next_view(&self.room)
                {
                    let offered_turn = self.offered.get(&seat).copied();
                    self.computer = Some(tokio::task::spawn_blocking(move || {
                        super::super::expert::choose(seat, policy, view, offered_turn)
                    }));
                }
                let _ = reply.send(result);
            }
        }
    }
}
