class SubscriptionPlansRepository < BaseRepository
  def model
    SubscriptionPlan
  end

  def free
    find_by_name!(model::FREE_PLAN)
  end

  def standard
    find_by_name!(model::STANDARD_PLAN)
  end

  def pro
    find_by_name!(model::PRO_PLAN)
  end

  def legendary
    find_by_name!(model::LEGENDARY_PLAN)
  end

  private

  def plan_cache
    @plan_cache ||= model.all.map { |sp| [sp.name, sp] }.to_h
  end

  def find_by_name!(name)
    plan_cache.fetch(name)
  end
end
