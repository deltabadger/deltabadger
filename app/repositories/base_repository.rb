class BaseRepository
  def save(object)
    object.save!
  end

  def find(id)
    model.find(id)
  end
end
