class SubscriptionPlansRepository < BaseRepository
  def model
    SubscriptionPlan
  end

  def saver
    find_by_name!(model::SAVER)
  end

  def investor
    find_by_name!(model::INVESTOR)
  end

  def hodler
    find_by_name!(model::HODLER)
  end

  private

  def plan_cache
    @plan_cache ||= model.all.map { |sp| [sp.name, sp] }.to_h
  end

  def find_by_name!(name)
    plan_cache.fetch(name)
  end
end
