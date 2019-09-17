class BotsRepository < BaseRepository
  def by_id_for_user(user, id)
    user.bots.find(id)
  end

  def model
    Bot
  end
end
